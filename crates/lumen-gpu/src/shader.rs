//! HLSL → SPIR-V → naga IR (ADR-003), with a content-addressed disk cache and
//! uniform-buffer reflection.
//!
//! DXC is an external tool for now: `LUMEN_DXC`, then `dxc/dxc.exe` or
//! `dxc.exe` beside the running executable, then the repository's
//! `.tools/dxc` above it (development builds), then `dxc` on `PATH`. Compiled
//! SPIR-V is cached by a hash of (compiler flags, entry, profile, source), so
//! DXC only runs the first time a shader variant is seen.

pub use naga;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("DXC not found (set LUMEN_DXC to dxc.exe)")]
    NoCompiler,
    #[error("DXC failed to start: {0}")]
    Launch(std::io::Error),
    #[error("DXC rejected {stage} shader: {message}")]
    Compile { stage: Stage, message: String },
    #[error("SPIR-V parse: {0}")]
    Parse(String),
    #[error("shader I/O: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
        })
    }
}

impl Stage {
    fn profile(self) -> &'static str {
        match self {
            Self::Vertex => "vs_6_0",
            Self::Fragment => "ps_6_0",
        }
    }
}

/// Part of the cache key, so changed flags never reuse stale entries.
const FLAGS: [&str; 5] = [
    "-spirv",
    "-HV",
    "2018",
    "-fspv-target-env=vulkan1.1",
    // Strict std140 cbuffer layout; naga rejects DXC's relaxed default.
    "-fvk-use-gl-layout",
];

/// DXC keeps HLSL's clip space (y up) unless `-fvk-invert-y`, which is what
/// wgpu expects, so naga must not apply its Vulkan y-flip.
const SPV_OPTIONS: naga::front::spv::Options = naga::front::spv::Options {
    adjust_coordinate_space: false,
    strict_capabilities: false,
    block_ctx_dump_prefix: None,
};

#[derive(Debug, Clone)]
pub struct ShaderCompiler {
    dxc: PathBuf,
    cache_dir: Option<PathBuf>,
    /// `dxc --version` output: part of every cache key so a compiler upgrade
    /// never reuses SPIR-V from another version (ADR-003).
    version: String,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How DXC processes are started, set once by the program (lumend hides
/// their console windows; that is OS code this crate does not hold).
static PROCESS_HOOK: std::sync::OnceLock<fn(&mut Command)> = std::sync::OnceLock::new();

/// Adjusts every DXC process before it starts (first call wins).
pub fn set_process_hook(hook: fn(&mut Command)) {
    let _ = PROCESS_HOOK.set(hook);
}

fn dxc_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if let Some(hook) = PROCESS_HOOK.get() {
        hook(&mut cmd);
    }
    cmd
}

impl ShaderCompiler {
    pub fn new(dxc: PathBuf, cache_dir: Option<PathBuf>) -> Self {
        let version = dxc_command(&dxc)
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        Self {
            dxc,
            cache_dir,
            version,
        }
    }

    /// Finds DXC (see module docs).
    pub fn locate(cache_dir: Option<PathBuf>) -> Result<Self, ShaderError> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = std::env::var_os("LUMEN_DXC") {
            candidates.push(p.into());
        }
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            for name in ["dxc", "dxc.exe"] {
                candidates.push(dir.join("dxc").join(name));
                candidates.push(dir.join(name));
            }
            // Development builds (`target/<profile>/`): the repository's
            // pinned copy from scripts/fetch-dxc.
            for up in dir.ancestors().skip(1).take(3) {
                for name in ["dxc", "dxc.exe"] {
                    candidates.push(up.join(".tools").join("dxc").join(name));
                }
            }
        }
        if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
            return Ok(Self::new(found, cache_dir));
        }
        // Last resort: PATH.
        match dxc_command("dxc").arg("--version").output() {
            Ok(out) if out.status.success() => Ok(Self::new("dxc".into(), cache_dir)),
            _ => Err(ShaderError::NoCompiler),
        }
    }

    /// Compiles one HLSL entry point to a naga module (cached).
    pub fn compile(
        &self,
        source: &str,
        entry: &str,
        stage: Stage,
    ) -> Result<naga::Module, ShaderError> {
        let spirv = self.spirv(source, entry, stage)?;
        naga::front::spv::parse_u8_slice(&spirv, &SPV_OPTIONS)
            .map_err(|e| ShaderError::Parse(e.to_string()))
    }

    fn spirv(&self, source: &str, entry: &str, stage: Stage) -> Result<Vec<u8>, ShaderError> {
        let key = cache_key(&self.version, source, entry, stage);
        let cached = self
            .cache_dir
            .as_ref()
            .map(|d| d.join(format!("{key:016x}.spv")));
        if let Some(bytes) = cached.as_ref().and_then(|p| std::fs::read(p).ok()) {
            return Ok(bytes);
        }

        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = std::env::temp_dir().join(format!("lumen-dxc-{}-{n}", std::process::id()));
        let input = stem.with_extension("hlsl");
        let output = stem.with_extension("spv");
        std::fs::write(&input, source)?;
        let run = dxc_command(&self.dxc)
            .args(FLAGS)
            .args(["-T", stage.profile(), "-E", entry])
            .arg("-Fo")
            .arg(&output)
            .arg(&input)
            .output();
        let _ = std::fs::remove_file(&input);
        let run = run.map_err(ShaderError::Launch)?;
        if !run.status.success() {
            let _ = std::fs::remove_file(&output);
            let stderr = String::from_utf8_lossy(&run.stderr);
            let message = stderr
                .lines()
                .find(|l| l.contains("error:"))
                .map_or_else(|| stderr.trim().to_owned(), |l| l.trim().to_owned());
            return Err(ShaderError::Compile { stage, message });
        }
        let bytes = std::fs::read(&output)?;
        let _ = std::fs::remove_file(&output);

        if let Some(path) = cached {
            // Best effort, atomic: a torn cache file would fail SPIR-V parsing later.
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension(format!("tmp{n}"));
            if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        Ok(bytes)
    }
}

/// FNV-1a 64 over everything that affects the output.
fn cache_key(version: &str, source: &str, entry: &str, stage: Stage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let parts = FLAGS
        .iter()
        .copied()
        .chain([version, stage.profile(), entry, source]);
    for part in parts {
        for b in part.bytes().chain([0]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// What a uniform-buffer member looks like in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformMember {
    pub name: String,
    pub offset: u32,
    pub kind: ScalarKind,
    /// Scalars per column (1 for scalars, n for vectors, rows for matrices).
    pub rows: u32,
    /// Columns for matrices, else 1.
    pub columns: u32,
    /// Bytes between matrix columns.
    pub column_stride: u32,
    /// Array length (1 if not an array) and byte stride between elements.
    pub count: u32,
    pub stride: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Float,
    Sint,
    Uint,
    Bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UniformLayout {
    pub size: u32,
    pub members: Vec<UniformMember>,
}

impl UniformLayout {
    /// Reflects the uniform struct at `(group, binding)`.
    pub fn reflect(module: &naga::Module, group: u32, binding: u32) -> Option<Self> {
        let (_, var) = module.global_variables.iter().find(|(_, v)| {
            v.space == naga::AddressSpace::Uniform
                && v.binding
                    .as_ref()
                    .is_some_and(|b| b.group == group && b.binding == binding)
        })?;
        let ty = &module.types[var.ty];
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            return None;
        };
        // wgpu validates bindings against naga's layout of the type, which can
        // exceed the declared span (std140 padding of trailing arrays).
        let mut layouter = naga::proc::Layouter::default();
        let laid_out = layouter
            .update(module.to_ctx())
            .ok()
            .map_or(0, |()| layouter[var.ty].size);
        let mut out = Self {
            size: (*span).max(laid_out),
            members: Vec::new(),
        };
        for m in members {
            if let Some(member) = describe(module, m) {
                out.members.push(member);
            }
        }
        Some(out)
    }

    pub fn member(&self, name: &str) -> Option<&UniformMember> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Writes `values` into `buf`; matrices take consecutive naga columns.
    /// Extra values are ignored; missing ones leave the buffer untouched.
    pub fn write(&self, buf: &mut [u8], name: &str, values: &[f32]) -> bool {
        let Some(m) = self.member(name) else {
            return false;
        };
        let per_element = (m.rows * m.columns) as usize;
        for (i, v) in values.iter().enumerate() {
            let element = (i / per_element.max(1)) as u32;
            let within = (i % per_element.max(1)) as u32;
            if element >= m.count {
                break;
            }
            let (col, row) = (within / m.rows, within % m.rows);
            let offset = (m.offset + element * m.stride + col * m.column_stride + row * 4) as usize;
            let bytes = match m.kind {
                ScalarKind::Float => v.to_le_bytes(),
                ScalarKind::Sint => (*v as i32).to_le_bytes(),
                ScalarKind::Uint | ScalarKind::Bool => (v.max(0.0) as u32).to_le_bytes(),
            };
            if let Some(dst) = buf.get_mut(offset..offset + 4) {
                dst.copy_from_slice(&bytes);
            }
        }
        true
    }
}

fn scalar_kind(kind: naga::ScalarKind) -> ScalarKind {
    match kind {
        naga::ScalarKind::Sint => ScalarKind::Sint,
        naga::ScalarKind::Uint => ScalarKind::Uint,
        naga::ScalarKind::Bool => ScalarKind::Bool,
        _ => ScalarKind::Float,
    }
}

fn describe(module: &naga::Module, m: &naga::StructMember) -> Option<UniformMember> {
    let mut ty = &module.types[m.ty].inner;
    let (mut count, mut stride) = (1, 0);
    if let naga::TypeInner::Array {
        base,
        size,
        stride: s,
    } = ty
    {
        count = match size {
            naga::ArraySize::Constant(n) => n.get(),
            _ => 1,
        };
        stride = *s;
        ty = &module.types[*base].inner;
    }
    let (kind, rows, columns, column_stride) = match ty {
        naga::TypeInner::Scalar(s) => (scalar_kind(s.kind), 1, 1, 0),
        naga::TypeInner::Vector { size, scalar } => (scalar_kind(scalar.kind), *size as u32, 1, 0),
        naga::TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => {
            // std140: each column padded to a vec4.
            (scalar_kind(scalar.kind), *rows as u32, *columns as u32, 16)
        }
        _ => return None,
    };
    Some(UniformMember {
        name: m.name.clone()?,
        offset: m.offset,
        kind,
        rows,
        columns,
        column_stride,
        count,
        stride,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_depends_on_every_input() {
        let a = cache_key("1.9", "x", "main", Stage::Vertex);
        assert_ne!(a, cache_key("1.9", "y", "main", Stage::Vertex));
        assert_ne!(a, cache_key("1.9", "x", "main2", Stage::Vertex));
        assert_ne!(a, cache_key("1.9", "x", "main", Stage::Fragment));
        assert_ne!(a, cache_key("1.10", "x", "main", Stage::Vertex));
        assert_eq!(a, cache_key("1.9", "x", "main", Stage::Vertex));
    }

    #[test]
    fn writes_scalars_vectors_and_arrays() {
        let layout = UniformLayout {
            size: 64,
            members: vec![
                UniformMember {
                    name: "t".into(),
                    offset: 0,
                    kind: ScalarKind::Float,
                    rows: 1,
                    columns: 1,
                    column_stride: 0,
                    count: 1,
                    stride: 0,
                },
                UniformMember {
                    name: "a".into(),
                    offset: 16,
                    kind: ScalarKind::Float,
                    rows: 2,
                    columns: 1,
                    column_stride: 0,
                    count: 2,
                    stride: 16,
                },
            ],
        };
        let mut buf = vec![0u8; 64];
        assert!(layout.write(&mut buf, "t", &[1.5]));
        assert!(layout.write(&mut buf, "a", &[1.0, 2.0, 3.0, 4.0, 9.0]));
        assert!(!layout.write(&mut buf, "missing", &[1.0]));
        let f = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(
            (f(0), f(16), f(20), f(32), f(36)),
            (1.5, 1.0, 2.0, 3.0, 4.0)
        );
    }
}
