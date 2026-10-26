//! Steam's `libraryfolders.vdf` (Valve KeyValues text): just enough to list
//! the library folders. Unknown structure is skipped, never an error.

use std::path::PathBuf;

/// The `path` of every library folder entry.
pub fn library_paths(text: &str) -> Vec<PathBuf> {
    let tokens = tokens(text);
    let mut paths = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Open => depth += 1,
            Token::Close => depth = depth.saturating_sub(1),
            // "libraryfolders" { "0" { "path" "…" } }
            Token::Text(key) if depth == 2 && key.eq_ignore_ascii_case("path") => {
                if let Some(Token::Text(value)) = tokens.get(i + 1) {
                    paths.push(PathBuf::from(value));
                    i += 1;
                }
            }
            Token::Text(_) => {}
        }
        i += 1;
    }
    paths
}

#[derive(Debug, PartialEq)]
enum Token {
    Text(String),
    Open,
    Close,
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(Token::Open),
            '}' => out.push(Token::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        _ => s.push(c),
                    }
                }
                out.push(Token::Text(s));
            }
            '/' if chars.peek() == Some(&'/') => {
                // Comment to the end of the line.
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_library_paths() {
        let text = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"431960"		"826275581"
			"path"		"not a library"
		}
	}
	// a comment
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        assert_eq!(
            library_paths(text),
            [
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
        assert!(library_paths("{{{\"path\"").is_empty());
    }
}
