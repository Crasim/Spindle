//! Line diffs for showing changes: Myers' algorithm, shown as hunks with a few lines of
//! context, like `git diff` but without needing git or a repository.

/// One line of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Same(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// The shortest edit turning `a` into `b`, line by line.
pub fn lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Line<'a>> {
    // Common start and end first: most edits touch a small part of a file.
    let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let mut out: Vec<Line> = a[..start].iter().map(|l| Line::Same(l)).collect();
    out.extend(myers(am, bm));
    out.extend(a[a.len() - end..].iter().map(|l| Line::Same(l)));
    out
}

fn myers<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Line<'a>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 {
        return b.iter().map(|l| Line::Added(l)).collect();
    }
    if m == 0 {
        return a.iter().map(|l| Line::Removed(l)).collect();
    }
    let max = (n + m) as usize;
    // Very different large files: removing and adding everything is clear enough.
    if (n as usize).saturating_mul(m as usize) > 4_000_000 && max > 20_000 {
        let mut out: Vec<Line> = a.iter().map(|l| Line::Removed(l)).collect();
        out.extend(b.iter().map(|l| Line::Added(l)));
        return out;
    }
    let offset = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = vec![];
    'search: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let i = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                break 'search;
            }
            k += 2;
        }
    }
    // Walk back through the saved frontiers.
    let mut out = vec![];
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let k = x - y;
        let i = (k + offset) as usize;
        let prev_k = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            out.push(Line::Same(a[x as usize]));
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                out.push(Line::Added(b[y as usize]));
            } else {
                x -= 1;
                out.push(Line::Removed(a[x as usize]));
            }
        }
    }
    out.reverse();
    out
}

/// A readable diff: hunks headed "@@ line N @@", changed lines as "- " and "+ ", and
/// `context` unchanged lines around each change as "  ".
pub fn unified(old: &str, new: &str, context: usize) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let all = lines(&a, &b);
    let changed: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, l)| !matches!(l, Line::Same(_)))
        .map(|(i, _)| i)
        .collect();
    let mut out = String::new();
    let mut i = 0;
    while i < changed.len() {
        // A hunk runs while changes are close enough for their contexts to touch.
        let first = changed[i];
        let mut last = first;
        while i + 1 < changed.len() && changed[i + 1] <= last + 2 * context + 1 {
            i += 1;
            last = changed[i];
        }
        i += 1;
        let from = first.saturating_sub(context);
        let to = (last + context + 1).min(all.len());
        let new_line = 1 + all[..from]
            .iter()
            .filter(|l| !matches!(l, Line::Removed(_)))
            .count();
        out.push_str(&format!("@@ line {new_line} @@\n"));
        for l in &all[from..to] {
            match l {
                Line::Same(s) => out.push_str(&format!("  {s}\n")),
                Line::Removed(s) => out.push_str(&format!("- {s}\n")),
                Line::Added(s) => out.push_str(&format!("+ {s}\n")),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_edits_become_separate_hunks() {
        let old: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 30\n", "line 30\nadded after 30\n");
        let d = unified(&old, &new, 2);
        assert_eq!(d.matches("@@").count(), 4, "two hunks:\n{d}");
        assert!(
            d.contains(
                "@@ line 3 @@\n  line 3\n  line 4\n- line 5\n+ line five\n  line 6\n  line 7\n"
            ),
            "{d}"
        );
        assert!(
            d.contains("  line 30\n+ added after 30\n  line 31\n"),
            "{d}"
        );
        assert!(!d.contains("line 15"), "unchanged middle isn't shown:\n{d}");
    }

    #[test]
    fn diffs_rebuild_the_new_text() {
        let a = ["a", "b", "c", "a", "b", "b", "a"];
        let b = ["c", "b", "a", "b", "a", "c"];
        let d = lines(&a, &b);
        let rebuilt: Vec<&str> = d
            .iter()
            .filter_map(|l| match l {
                Line::Same(s) | Line::Added(s) => Some(*s),
                Line::Removed(_) => None,
            })
            .collect();
        let original: Vec<&str> = d
            .iter()
            .filter_map(|l| match l {
                Line::Same(s) | Line::Removed(s) => Some(*s),
                Line::Added(_) => None,
            })
            .collect();
        assert_eq!(rebuilt, b);
        assert_eq!(original, a);
        // The classic example's shortest edit has 5 changes.
        assert_eq!(d.iter().filter(|l| !matches!(l, Line::Same(_))).count(), 5);
        assert_eq!(unified("same\n", "same\n", 3), "");
    }
}
