//! `wpdump shader-check`: ADR-003 spike harness.
//!
//! Translates every vertex/fragment pair (WE built-ins + every corpus pkg) and
//! checks it can reach wgpu:
//! - `glsl`: GLSL 450 → naga glsl-in → validate
//! - `hlsl`: HLSL → DXC (`-spirv`) → naga spv-in → validate
//!
//! Reports the pass rate and groups failures by (masked) message.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use diorama_core::Limits;
use diorama_we::Pkg;
use diorama_we::shader::{self, GlslPair, HlslPair};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    Glsl,
    Hlsl,
}

#[derive(Debug)]
pub struct Options<'a> {
    pub corpus: &'a Path,
    pub install: &'a Path,
    pub dump: Option<&'a Path>,
    pub all_combos_on: bool,
    pub backend: Backend,
    pub dxc: Option<&'a Path>,
}

struct PairSource {
    label: String,
    vertex: String,
    fragment: String,
    /// pkg entries (`shaders/...`) consulted before the WE install.
    pkg_shaders: HashMap<String, String>,
}

enum Outcome {
    Ok,
    Fail { kind: &'static str, detail: String },
}

fn fail(kind: &'static str, detail: impl Into<String>) -> Outcome {
    Outcome::Fail {
        kind,
        detail: detail.into(),
    }
}

pub fn run(opts: &Options<'_>) -> Result<String> {
    if opts.backend == Backend::Hlsl && opts.dxc.is_none() {
        bail!("--backend hlsl needs --dxc <path to dxc.exe>");
    }
    let shaders_root = opts.install.join("assets").join("shaders");
    let mut sources = builtin_pairs(&opts.install.join("assets"));
    sources.extend(corpus_pairs(opts.corpus));

    // Identical pairs appear in many pkgs; check each distinct pair once.
    let mut seen = HashSet::new();
    sources.retain(|s| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&s.vertex, &s.fragment).hash(&mut h);
        seen.insert(h.finish())
    });

    let work_dir =
        std::env::temp_dir().join(format!("diorama-shader-check-{}", std::process::id()));
    fs::create_dir_all(&work_dir)?;
    if let Some(dir) = opts.dump {
        fs::create_dir_all(dir)?;
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome, Vec<String>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(src) = sources.get(index) else { break };
                    let (outcome, requires) = check_one(index, src, &shaders_root, &work_dir, opts);
                    if let Ok(mut r) = results.lock() {
                        r.push((index, outcome, requires));
                    }
                }
            });
        }
    });
    let _ = fs::remove_dir_all(&work_dir);

    let mut ok = 0;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut requires: BTreeMap<String, usize> = BTreeMap::new();
    for (index, outcome, reqs) in results.into_inner().unwrap_or_default() {
        for r in reqs {
            *requires.entry(r).or_default() += 1;
        }
        match outcome {
            Outcome::Ok => ok += 1,
            Outcome::Fail { kind, detail } => {
                let first = detail.lines().next().unwrap_or("").trim();
                let key = format!(
                    "{kind}: {}",
                    mask(first).chars().take(110).collect::<String>()
                );
                groups
                    .entry(key)
                    .or_default()
                    .push(sources[index].label.clone());
            }
        }
    }

    let total = sources.len();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Shader check ({})\n\nVariant: {}\n\n**{ok}/{total} distinct pairs pass ({:.1}%)**\n",
        match opts.backend {
            Backend::Glsl => "GLSL 450 → naga",
            Backend::Hlsl => "HLSL → DXC SPIR-V → naga",
        },
        if opts.all_combos_on {
            "all combos = 1"
        } else {
            "default combos"
        },
        100.0 * ok as f64 / total.max(1) as f64
    );
    let _ = writeln!(out, "## `#require` directives\n");
    for (r, n) in &requires {
        let _ = writeln!(out, "- `{r}`: {n}");
    }
    let _ = writeln!(
        out,
        "\n## Failures by cause\n\n| count | cause | example |\n|---:|---|---|"
    );
    let mut rows: Vec<_> = groups.iter().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.1.len()));
    for (cause, labels) in rows {
        let _ = writeln!(
            out,
            "| {} | {} | `{}` |",
            labels.len(),
            cause.replace('|', "\\|"),
            labels[0]
        );
    }
    Ok(out)
}

fn check_one(
    index: usize,
    src: &PairSource,
    shaders_root: &Path,
    work_dir: &Path,
    opts: &Options<'_>,
) -> (Outcome, Vec<String>) {
    let resolve = |name: &str| -> Option<String> {
        src.pkg_shaders
            .get(&format!("shaders/{name}"))
            .cloned()
            .or_else(|| fs::read_to_string(shaders_root.join(name)).ok())
    };
    let mut overrides = BTreeMap::new();
    if opts.all_combos_on {
        let combined = format!("{}\n{}", src.vertex, src.fragment);
        for c in shader::decl::parse_combos(&combined) {
            overrides.insert(c.name, 1);
        }
    }
    let prepared = match shader::prepare_pair(
        &src.vertex,
        &src.fragment,
        &resolve,
        &shader::engine_defines(),
        &overrides,
        &shader::EngineContext::default(),
    ) {
        Ok(p) => p,
        Err(e) => return (fail("prepare", e.to_string()), Vec::new()),
    };
    let requires = prepared.requires.clone();

    let (outcome, texts) = match opts.backend {
        Backend::Glsl => match prepared.glsl() {
            Ok(g) => (check_glsl(&g), [g.vertex, g.fragment]),
            Err(e) => (fail("translate", e.to_string()), Default::default()),
        },
        Backend::Hlsl => match prepared.hlsl() {
            Ok(h) => {
                let dxc = opts.dxc.unwrap_or(Path::new("dxc"));
                (check_hlsl(&h, dxc, work_dir, index), [h.vertex, h.fragment])
            }
            Err(e) => (fail("translate", e.to_string()), Default::default()),
        },
    };

    if let (Outcome::Fail { detail, .. }, Some(dir)) = (&outcome, opts.dump) {
        let ext = if opts.backend == Backend::Glsl {
            ["vert", "frag"]
        } else {
            ["vs.hlsl", "ps.hlsl"]
        };
        let _ = fs::write(dir.join(format!("{index:04}.{}", ext[0])), &texts[0]);
        let _ = fs::write(dir.join(format!("{index:04}.{}", ext[1])), &texts[1]);
        let _ = fs::write(
            dir.join(format!("{index:04}.txt")),
            format!("{}\n{detail}\n", src.label),
        );
    }
    (outcome, requires)
}

fn validate(module: &naga::Module, stage: &str, src: Option<&str>) -> Outcome {
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    match validator.validate(module) {
        Ok(_) => Outcome::Ok,
        Err(e) => {
            let snippet = src
                .and_then(|s| {
                    e.spans()
                        .filter_map(|(span, _)| span.to_range())
                        .last()
                        .map(|r| excerpt(s, r))
                })
                .unwrap_or_default();
            fail(
                if stage == "vertex" {
                    "validate vertex"
                } else {
                    "validate fragment"
                },
                format!("{:?}\n{snippet}", e.into_inner()),
            )
        }
    }
}

fn check_glsl(g: &GlslPair) -> Outcome {
    use naga::ShaderStage;
    for (stage, name, src) in [
        (ShaderStage::Vertex, "vertex", &g.vertex),
        (ShaderStage::Fragment, "fragment", &g.fragment),
    ] {
        let mut frontend = naga::front::glsl::Frontend::default();
        let module = match frontend.parse(&naga::front::glsl::Options::from(stage), src) {
            Ok(m) => m,
            Err(e) => {
                let snippet = e
                    .errors
                    .first()
                    .and_then(|err| err.meta.to_range())
                    .map(|r| excerpt(src, r))
                    .unwrap_or_default();
                let kind = if name == "vertex" {
                    "parse vertex"
                } else {
                    "parse fragment"
                };
                return fail(kind, format!("{e}\n{snippet}"));
            }
        };
        if let f @ Outcome::Fail { .. } = validate(&module, name, Some(src)) {
            return f;
        }
    }
    Outcome::Ok
}

fn check_hlsl(h: &HlslPair, dxc: &Path, work_dir: &Path, index: usize) -> Outcome {
    for (name, profile, entry, src) in [
        ("vertex", "vs_6_0", h.vertex_entry, &h.vertex),
        ("fragment", "ps_6_0", h.fragment_entry, &h.fragment),
    ] {
        let input = work_dir.join(format!("{index}-{name}.hlsl"));
        let output = work_dir.join(format!("{index}-{name}.spv"));
        if let Err(e) = fs::write(&input, src) {
            return fail("io", e.to_string());
        }
        let run = Command::new(dxc)
            .args([
                "-spirv",
                "-HV",
                "2018",
                "-fspv-target-env=vulkan1.1",
                // Strict std140 cbuffer layout; naga rejects DXC's relaxed default.
                "-fvk-use-gl-layout",
                "-T",
                profile,
                "-E",
                entry,
            ])
            .arg("-Fo")
            .arg(&output)
            .arg(&input)
            .output();
        let run = match run {
            Ok(r) => r,
            Err(e) => return fail("dxc launch", e.to_string()),
        };
        if !run.status.success() {
            let stderr = String::from_utf8_lossy(&run.stderr);
            // First "error:" line, without the temp-file path prefix.
            let first = stderr
                .lines()
                .find(|l| l.contains("error:"))
                .map(|l| {
                    l.split_once("error:")
                        .map_or(l, |(_, m)| m)
                        .trim()
                        .to_owned()
                })
                .unwrap_or_else(|| stderr.lines().next().unwrap_or("").to_owned());
            let kind = if name == "vertex" {
                "dxc vertex"
            } else {
                "dxc fragment"
            };
            return fail(kind, format!("{first}\n{stderr}"));
        }
        let spv = match fs::read(&output) {
            Ok(b) => b,
            Err(e) => return fail("io", e.to_string()),
        };
        let module =
            match naga::front::spv::parse_u8_slice(&spv, &naga::front::spv::Options::default()) {
                Ok(m) => m,
                Err(e) => {
                    let kind = if name == "vertex" {
                        "spv-in vertex"
                    } else {
                        "spv-in fragment"
                    };
                    return fail(kind, e.to_string());
                }
            };
        if let f @ Outcome::Fail { .. } = validate(&module, name, None) {
            return f;
        }
    }
    Outcome::Ok
}

/// The full source line(s) covering a byte range.
fn excerpt(src: &str, range: std::ops::Range<usize>) -> String {
    let start = src[..range.start.min(src.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let end = src[range.end.min(src.len())..]
        .find('\n')
        .map_or(src.len(), |i| range.end + i);
    format!(">> {}", src.get(start..end).unwrap_or("").trim())
}

fn mask(s: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        if c == '"' || c == '`' || c == '\'' {
            in_quote = !in_quote;
            out.push(c);
        } else if in_quote {
            if !out.ends_with('…') {
                out.push('…');
            }
        } else if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn builtin_pairs(assets: &Path) -> Vec<PairSource> {
    let mut verts = Vec::new();
    walk(assets, &mut verts);
    let mut out = Vec::new();
    for vert in verts {
        if vert.components().any(|c| c.as_os_str() == "HLSL") {
            continue; // pure HLSL playlist shaders, not the WE dialect
        }
        let frag = vert.with_extension("frag");
        let (Ok(vertex), Ok(fragment)) = (fs::read_to_string(&vert), fs::read_to_string(&frag))
        else {
            continue;
        };
        out.push(PairSource {
            label: format!(
                "builtin:{}",
                vert.strip_prefix(assets).unwrap_or(&vert).display()
            ),
            vertex,
            fragment,
            pkg_shaders: HashMap::new(),
        });
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "vert") {
            out.push(p);
        }
    }
}

fn corpus_pairs(corpus: &Path) -> Vec<PairSource> {
    let limits = Limits::default();
    let mut out = Vec::new();
    let Ok(dirs) = fs::read_dir(corpus) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(bytes) = fs::read(dir.path().join("scene.pkg")) else {
            continue;
        };
        let Ok(pkg) = Pkg::parse(&bytes, &limits) else {
            continue;
        };
        let pkg_shaders: HashMap<String, String> = pkg
            .entries()
            .iter()
            .filter(|e| e.path.starts_with("shaders/"))
            .filter_map(|e| {
                Some((
                    e.path.clone(),
                    String::from_utf8_lossy(pkg.data(e)?).into_owned(),
                ))
            })
            .collect();
        for (path, vertex) in &pkg_shaders {
            let Some(stem) = path.strip_suffix(".vert") else {
                continue;
            };
            let Some(fragment) = pkg_shaders.get(&format!("{stem}.frag")) else {
                continue;
            };
            out.push(PairSource {
                label: format!("{}:{path}", dir.file_name().to_string_lossy()),
                vertex: vertex.clone(),
                fragment: fragment.clone(),
                pkg_shaders: pkg_shaders.clone(),
            });
        }
    }
    out
}
