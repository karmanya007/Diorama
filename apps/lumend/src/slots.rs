//! Which wallpaper shows on which monitor. Each wallpaper claims output ids;
//! one claiming none takes every monitor no other wallpaper claims.

use std::collections::HashSet;

use lumen_platform::OutputInfo;

/// The monitors each wallpaper shows on, given what each claims.
pub fn assign(claims: &[Vec<String>], outputs: &[OutputInfo]) -> Vec<Vec<OutputInfo>> {
    let claimed: HashSet<&str> = claims.iter().flatten().map(String::as_str).collect();
    claims
        .iter()
        .map(|claim| {
            outputs
                .iter()
                .filter(|o| {
                    if claim.is_empty() {
                        !claimed.contains(o.id.0.as_str())
                    } else {
                        claim.contains(&o.id.0)
                    }
                })
                .cloned()
                .collect()
        })
        .collect()
}

/// Output ids for what the user typed: an id or a device name
/// (`\\.\DISPLAY1`, any case).
pub fn resolve(names: &[String], outputs: &[OutputInfo]) -> Result<Vec<String>, String> {
    names
        .iter()
        .map(|name| {
            outputs
                .iter()
                .find(|o| o.id.0 == *name || o.device.eq_ignore_ascii_case(name))
                .map(|o| o.id.0.clone())
                .ok_or_else(|| format!("no monitor {name} (`lumen status` lists them)"))
        })
        .collect()
}

/// Claims left after a new wallpaper claims `new` (empty: every monitor):
/// `None` for wallpapers that no longer show anywhere.
pub fn reclaim(existing: &[Vec<String>], new: &[String]) -> Vec<Option<Vec<String>>> {
    existing
        .iter()
        .map(|claim| {
            if new.is_empty() {
                return None;
            }
            if claim.is_empty() {
                // Still covers the monitors nobody claims.
                return Some(Vec::new());
            }
            let left: Vec<String> = claim.iter().filter(|o| !new.contains(o)).cloned().collect();
            (!left.is_empty()).then_some(left)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_platform::OutputId;

    fn output(id: &str, device: &str) -> OutputInfo {
        OutputInfo {
            id: OutputId(id.into()),
            device: device.into(),
            ..OutputInfo::default()
        }
    }

    fn ids(group: &[OutputInfo]) -> Vec<&str> {
        group.iter().map(|o| o.id.0.as_str()).collect()
    }

    #[test]
    fn explicit_claims_win_and_the_rest_takes_the_others() {
        let outputs = [output("A", "D1"), output("B", "D2"), output("C", "D3")];
        let groups = assign(&[Vec::new(), vec!["B".into()]], &outputs);
        assert_eq!(ids(&groups[0]), ["A", "C"]);
        assert_eq!(ids(&groups[1]), ["B"]);
        // An unplugged monitor's claim shows nothing until it returns.
        let groups = assign(&[vec!["Z".into()]], &outputs);
        assert!(groups[0].is_empty());
    }

    #[test]
    fn names_resolve_to_ids() {
        let outputs = [output("DEL-1", r"\\.\DISPLAY1")];
        assert_eq!(
            resolve(&[r"\\.\display1".into()], &outputs).unwrap(),
            ["DEL-1"]
        );
        assert_eq!(resolve(&["DEL-1".into()], &outputs).unwrap(), ["DEL-1"]);
        assert!(resolve(&["X".into()], &outputs).is_err());
    }

    #[test]
    fn new_claims_take_monitors_from_others() {
        let existing = [Vec::new(), vec!["A".into(), "B".into()], vec!["C".into()]];
        assert_eq!(
            reclaim(&existing, &["B".into(), "C".into()]),
            [Some(Vec::new()), Some(vec!["A".into()]), None]
        );
        assert_eq!(reclaim(&existing, &[]), [None, None, None]);
    }
}
