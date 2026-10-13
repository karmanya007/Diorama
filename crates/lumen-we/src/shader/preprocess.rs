//! Partial C preprocessor for WE shaders.
//!
//! Resolves `#include` (include-once) and evaluates `#if`/`#ifdef`/`#elif`/
//! `#else`/`#endif` against combo values and engine defines. `#define` lines in
//! active regions are kept in the output (the downstream compiler expands macro
//! uses) and also recorded so later conditionals can test them. Inactive lines
//! become blank so line numbers in compiler errors still match.

use std::collections::{HashMap, HashSet};

use crate::error::{Error, Result};

const MAX_INCLUDE_DEPTH: usize = 16;
const MAX_MACRO_DEPTH: usize = 32;

/// Expands `#include "name"` recursively. Each file is included at most once.
pub fn expand_includes(source: &str, resolve: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let mut seen = HashSet::new();
    let mut out = String::with_capacity(source.len());
    expand_into(source, resolve, &mut seen, &mut out, 0)?;
    Ok(out)
}

fn expand_into(
    source: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    seen: &mut HashSet<String>,
    out: &mut String,
    depth: usize,
) -> Result<()> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(shader_err("include depth limit exceeded"));
    }
    for line in source.lines() {
        if let Some(name) = include_target(line) {
            if seen.insert(name.to_owned()) {
                let text = resolve(name)
                    .ok_or_else(|| shader_err(format!("include not found: {name}")))?;
                expand_into(&text, resolve, seen, out, depth + 1)?;
            }
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(())
}

fn include_target(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("include")?.trim();
    rest.strip_prefix('"')?.split('"').next()
}

#[derive(Debug, Clone, Default)]
pub struct Conditionals {
    pub text: String,
    /// Values of `#require` directives (e.g. `LightingV1`).
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    /// This branch is emitting.
    active: bool,
    /// Some branch of this `#if` chain has already been taken.
    taken: bool,
    /// The enclosing region is emitting.
    parent_active: bool,
}

/// Evaluates conditionals. `defines` seeds the macro table (combos, engine flags).
pub fn evaluate_conditionals(
    source: &str,
    defines: &HashMap<String, String>,
) -> Result<Conditionals> {
    let mut macros: HashMap<String, Option<String>> = defines
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    let mut stack: Vec<Frame> = Vec::new();
    let mut out = Conditionals::default();

    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        // Join backslash continuations into one logical line.
        let mut logical = String::from(lines[i]);
        let mut consumed = 1;
        while logical.ends_with('\\') && i + consumed < lines.len() {
            logical.pop();
            logical.push_str(lines[i + consumed]);
            consumed += 1;
        }
        i += consumed;

        let active = stack.last().is_none_or(|f| f.active);
        let directive = logical
            .trim_start()
            .strip_prefix('#')
            .map(|d| d.trim_start());

        let emit = match directive {
            None => active,
            Some(d) => {
                let (name, rest) = split_directive(d);
                match name {
                    "if" | "ifdef" | "ifndef" => {
                        let cond = active
                            && match name {
                                "if" => eval_expr(rest, &macros)? != 0,
                                "ifdef" => macros.contains_key(first_word(rest)),
                                _ => !macros.contains_key(first_word(rest)),
                            };
                        stack.push(Frame {
                            active: cond,
                            taken: cond,
                            parent_active: active,
                        });
                        false
                    }
                    "elif" => {
                        let f = stack
                            .last_mut()
                            .ok_or_else(|| shader_err("#elif without #if"))?;
                        let cond = f.parent_active && !f.taken && eval_expr(rest, &macros)? != 0;
                        f.active = cond;
                        f.taken |= cond;
                        false
                    }
                    "else" => {
                        let f = stack
                            .last_mut()
                            .ok_or_else(|| shader_err("#else without #if"))?;
                        f.active = f.parent_active && !f.taken;
                        f.taken = true;
                        false
                    }
                    "endif" => {
                        stack
                            .pop()
                            .ok_or_else(|| shader_err("#endif without #if"))?;
                        false
                    }
                    "define" if active => {
                        let (macro_name, body) = parse_define(rest);
                        macros.insert(macro_name.to_owned(), body);
                        true
                    }
                    "undef" if active => {
                        macros.remove(first_word(rest));
                        true
                    }
                    "require" if active => {
                        out.requires.push(first_word(rest).to_owned());
                        false
                    }
                    // `#version` / `#extension` are replaced by the translator's header.
                    "version" | "extension" => false,
                    _ => active,
                }
            }
        };

        if emit {
            out.text.push_str(&logical);
        }
        for _ in 0..consumed {
            out.text.push('\n');
        }
    }

    if !stack.is_empty() {
        return Err(shader_err("unterminated #if"));
    }
    Ok(out)
}

fn split_directive(d: &str) -> (&str, &str) {
    let end = d
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(d.len());
    (&d[..end], d[end..].trim())
}

fn first_word(s: &str) -> &str {
    s.split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
}

/// `NAME body` → (NAME, Some(body)); function-like macros → (NAME, None).
fn parse_define(rest: &str) -> (&str, Option<String>) {
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    let (name, tail) = rest.split_at(end);
    if tail.starts_with('(') {
        (name, None)
    } else {
        let body = strip_line_comment(tail).trim();
        (name, Some(body.to_owned()))
    }
}

fn strip_line_comment(s: &str) -> &str {
    s.split("//").next().unwrap_or(s)
}

fn shader_err(msg: impl Into<String>) -> Error {
    Error::Shader(msg.into())
}

// ---------------------------------------------------------------------------
// #if expression evaluation

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(i64),
    Ident(String),
    Op(&'static str),
}

const OPS: [&str; 24] = [
    "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "(", ")", "!", "~", "*", "/", "%", "+", "-",
    "<", ">", "&", "^", "|", "?", ":",
];

fn tokenize(expr: &str) -> Result<Vec<Tok>> {
    let s = strip_line_comment(expr);
    let bytes = s.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii() {
            return Err(shader_err("non-ASCII character in #if"));
        }
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
                i += 1;
            }
            let lit = s[start..i].trim_end_matches(['u', 'U', 'l', 'L']);
            let value =
                if let Some(hex) = lit.strip_prefix("0x").or_else(|| lit.strip_prefix("0X")) {
                    i64::from_str_radix(hex, 16)
                } else {
                    lit.parse()
                }
                .map_err(|_| shader_err(format!("bad number in #if: {lit}")))?;
            toks.push(Tok::Num(value));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len()
                && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            toks.push(Tok::Ident(s[start..i].to_owned()));
        } else {
            let op = OPS
                .iter()
                .find(|op| s[i..].starts_with(**op))
                .ok_or_else(|| shader_err(format!("unexpected {c:?} in #if")))?;
            toks.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(toks)
}

/// Evaluates a `#if` expression. Undefined identifiers are 0, as in C.
pub(crate) fn eval_expr(expr: &str, macros: &HashMap<String, Option<String>>) -> Result<i64> {
    eval_depth(expr, macros, 0)
}

fn eval_depth(expr: &str, macros: &HashMap<String, Option<String>>, depth: usize) -> Result<i64> {
    if depth > MAX_MACRO_DEPTH {
        return Err(shader_err("macro expansion too deep in #if"));
    }
    let toks = tokenize(expr)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        macros,
        depth,
    };
    let v = p.ternary()?;
    if p.pos != toks.len() {
        return Err(shader_err(format!("trailing tokens in #if {expr:?}")));
    }
    Ok(v)
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    macros: &'a HashMap<String, Option<String>>,
    depth: usize,
}

impl Parser<'_> {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.pos) {
            Some(Tok::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn eat(&mut self, op: &str) -> bool {
        if self.peek_op() == Some(op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ternary(&mut self) -> Result<i64> {
        let cond = self.binary(0)?;
        if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err(shader_err("expected ':' in #if"));
            }
            let b = self.ternary()?;
            Ok(if cond != 0 { a } else { b })
        } else {
            Ok(cond)
        }
    }

    fn binary(&mut self, min_prec: u8) -> Result<i64> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.peek_op() {
            let Some(prec) = precedence(op) else { break };
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(prec + 1)?;
            lhs = apply(op, lhs, rhs)?;
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<i64> {
        if self.eat("!") {
            return Ok(i64::from(self.unary()? == 0));
        }
        if self.eat("-") {
            return Ok(self.unary()?.wrapping_neg());
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("~") {
            return Ok(!self.unary()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<i64> {
        if self.eat("(") {
            let v = self.ternary()?;
            if !self.eat(")") {
                return Err(shader_err("expected ')' in #if"));
            }
            return Ok(v);
        }
        match self.toks.get(self.pos).cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(n)
            }
            Some(Tok::Ident(name)) if name == "defined" => {
                self.pos += 1;
                let paren = self.eat("(");
                let Some(Tok::Ident(target)) = self.toks.get(self.pos).cloned() else {
                    return Err(shader_err("expected identifier after defined"));
                };
                self.pos += 1;
                if paren && !self.eat(")") {
                    return Err(shader_err("expected ')' after defined(X"));
                }
                Ok(i64::from(self.macros.contains_key(&target)))
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                match self.macros.get(&name) {
                    Some(Some(body)) if !body.is_empty() => {
                        eval_depth(body, self.macros, self.depth + 1)
                    }
                    _ => Ok(0),
                }
            }
            other => Err(shader_err(format!("unexpected token in #if: {other:?}"))),
        }
    }
}

fn precedence(op: &str) -> Option<u8> {
    Some(match op {
        "||" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        _ => return None,
    })
}

fn apply(op: &str, a: i64, b: i64) -> Result<i64> {
    Ok(match op {
        "||" => i64::from(a != 0 || b != 0),
        "&&" => i64::from(a != 0 && b != 0),
        "|" => a | b,
        "^" => a ^ b,
        "&" => a & b,
        "==" => i64::from(a == b),
        "!=" => i64::from(a != b),
        "<" => i64::from(a < b),
        ">" => i64::from(a > b),
        "<=" => i64::from(a <= b),
        ">=" => i64::from(a >= b),
        "<<" => a.wrapping_shl(b as u32),
        ">>" => a.wrapping_shr(b as u32),
        "+" => a.wrapping_add(b),
        "-" => a.wrapping_sub(b),
        "*" => a.wrapping_mul(b),
        "/" | "%" if b == 0 => return Err(shader_err("division by zero in #if")),
        "/" => a.wrapping_div(b),
        "%" => a.wrapping_rem(b),
        _ => return Err(shader_err(format!("unknown operator {op}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn eval(expr: &str, pairs: &[(&str, &str)]) -> i64 {
        let macros = defs(pairs).into_iter().map(|(k, v)| (k, Some(v))).collect();
        eval_expr(expr, &macros).unwrap()
    }

    #[test]
    fn expressions_follow_c_rules() {
        assert_eq!(eval("1 + 2 * 3", &[]), 7);
        assert_eq!(eval("(1 + 2) * 3", &[]), 9);
        assert_eq!(eval("MASK == 1", &[("MASK", "1")]), 1);
        assert_eq!(eval("UNDEFINED", &[]), 0);
        assert_eq!(eval("!UNDEFINED && 0x10 == 16", &[]), 1);
        assert_eq!(
            eval("FOG_DIST || FOG_HEIGHT || LIGHTING", &[("LIGHTING", "1")]),
            1
        );
        assert_eq!(eval("defined(A) + defined B", &[("A", "")]), 1);
        assert_eq!(eval("A ? 5 : 6", &[("A", "B"), ("B", "2")]), 5);
        assert_eq!(eval("1 << 3 | 1", &[]), 9);
        assert_eq!(eval("-1 < 0", &[]), 1);
    }

    #[test]
    fn conditionals_select_branches() {
        let src = "a\n#if MASK == 1\nb\n#elif MASK == 2\nc\n#else\nd\n#endif\ne\n";
        let out = evaluate_conditionals(src, &defs(&[("MASK", "2")])).unwrap();
        let lines: Vec<_> = out.text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["a", "c", "e"]);
        assert_eq!(out.text.lines().count(), src.lines().count());
    }

    #[test]
    fn nested_inactive_regions_stay_inactive() {
        let src = "#if 0\n#if 1\nx\n#else\ny\n#endif\n#else\nz\n#endif\n";
        let out = evaluate_conditionals(src, &HashMap::new()).unwrap();
        assert_eq!(out.text.trim(), "z");
    }

    #[test]
    fn defines_feed_later_conditionals_and_are_kept() {
        let src = "#define FOO 3\n#if FOO > 2\nyes\n#endif\n#require LightingV1\n";
        let out = evaluate_conditionals(src, &HashMap::new()).unwrap();
        assert!(out.text.contains("#define FOO 3"));
        assert!(out.text.contains("yes"));
        assert_eq!(out.requires, ["LightingV1"]);
    }

    #[test]
    fn includes_expand_once_and_report_missing() {
        let resolve = |name: &str| match name {
            "a.h" => Some("#include \"b.h\"\nA".to_owned()),
            "b.h" => Some("B".to_owned()),
            _ => None,
        };
        let out = expand_includes("#include \"a.h\"\n#include \"b.h\"\nmain", &resolve).unwrap();
        assert_eq!(out.matches('B').count(), 1);
        assert!(out.contains('A') && out.contains("main"));
        assert!(expand_includes("#include \"zzz.h\"", &resolve).is_err());
    }

    #[test]
    fn unbalanced_directives_are_errors() {
        assert!(evaluate_conditionals("#if 1\n", &HashMap::new()).is_err());
        assert!(evaluate_conditionals("#endif\n", &HashMap::new()).is_err());
        assert!(evaluate_conditionals("#else\n", &HashMap::new()).is_err());
    }
}
