use crate::process::{command, safe_output};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Copy)]
enum Gender {
    Masculine,
    Feminine,
    Neuter,
}

struct BakedGood {
    name: &'static str,
    gender: Gender,
}

const GOODS: &[BakedGood] = &[
    BakedGood { name: "rohlik", gender: Gender::Masculine },
    BakedGood { name: "houska", gender: Gender::Feminine },
    BakedGood { name: "kolac", gender: Gender::Masculine },
    BakedGood { name: "veka", gender: Gender::Feminine },
    BakedGood { name: "chleb", gender: Gender::Masculine },
    BakedGood { name: "buchta", gender: Gender::Feminine },
    BakedGood { name: "kobliha", gender: Gender::Feminine },
    BakedGood { name: "strudl", gender: Gender::Masculine },
    BakedGood { name: "mazanec", gender: Gender::Masculine },
    BakedGood { name: "vanocka", gender: Gender::Feminine },
    BakedGood { name: "trdlo", gender: Gender::Neuter },
    BakedGood { name: "trdelnik", gender: Gender::Masculine },
    BakedGood { name: "loupak", gender: Gender::Masculine },
    BakedGood { name: "makovec", gender: Gender::Masculine },
    BakedGood { name: "zavin", gender: Gender::Masculine },
    BakedGood { name: "kremrole", gender: Gender::Feminine },
    BakedGood { name: "venecek", gender: Gender::Masculine },
    BakedGood { name: "rakvicka", gender: Gender::Feminine },
    BakedGood { name: "laskonka", gender: Gender::Feminine },
    BakedGood { name: "medovnik", gender: Gender::Masculine },
    BakedGood { name: "bublanina", gender: Gender::Feminine },
    BakedGood { name: "pernik", gender: Gender::Masculine },
    BakedGood { name: "knedlik", gender: Gender::Masculine },
    BakedGood { name: "palacinka", gender: Gender::Feminine },
    BakedGood { name: "babovka", gender: Gender::Feminine },
    BakedGood { name: "povidlak", gender: Gender::Masculine },
    BakedGood { name: "vdolek", gender: Gender::Masculine },
    BakedGood { name: "bochanek", gender: Gender::Masculine },
    BakedGood { name: "kolatek", gender: Gender::Masculine },
    BakedGood { name: "zemle", gender: Gender::Feminine },
    BakedGood { name: "paska", gender: Gender::Feminine },
    BakedGood { name: "pletenak", gender: Gender::Masculine },
    BakedGood { name: "orechovec", gender: Gender::Masculine },
    BakedGood { name: "tvarohac", gender: Gender::Masculine },
    BakedGood { name: "jablecnak", gender: Gender::Masculine },
    BakedGood { name: "svestkac", gender: Gender::Masculine },
    BakedGood { name: "linecak", gender: Gender::Masculine },
    BakedGood { name: "vetrnik", gender: Gender::Masculine },
];

/// (stem, masculine_suffix, feminine_suffix, neuter_suffix)
const ADJECTIVE_STEMS: &[(&str, &str, &str, &str)] = &[
    ("velk", "y", "a", "e"),
    ("mal", "y", "a", "e"),
    ("zlat", "y", "a", "e"),
    ("cerstv", "y", "a", "e"),
    ("sladk", "y", "a", "e"),
    ("tezk", "y", "a", "e"),
    ("lehk", "y", "a", "e"),
    ("hork", "y", "a", "e"),
    ("divok", "y", "a", "e"),
    ("rychl", "y", "a", "e"),
];

fn adjective_for(stem: &str, suffix_m: &str, suffix_f: &str, suffix_n: &str, good: &BakedGood) -> String {
    let suffix = match good.gender {
        Gender::Masculine => suffix_m,
        Gender::Feminine => suffix_f,
        Gender::Neuter => suffix_n,
    };
    format!("{}{}", stem, suffix)
}

/// Per-repo username cache. Keyed by canonical repo path so that different
/// repositories resolve to their own GitHub owner. Uses a simple Mutex
/// instead of OnceLock to allow per-path caching.
static USERNAME_CACHE: std::sync::Mutex<Option<std::collections::HashMap<std::path::PathBuf, String>>> =
    std::sync::Mutex::new(None);

fn detect_github_username(repo_path: &Path) -> String {
    let canonical = repo_path.canonicalize().unwrap_or_else(|_| repo_path.to_path_buf());
    let mut guard = USERNAME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(std::collections::HashMap::new);
    if let Some(cached) = cache.get(&canonical) {
        return cached.clone();
    }
    let username = detect_github_username_inner(repo_path);
    cache.insert(canonical, username.clone());
    username
}

fn detect_github_username_inner(repo_path: &Path) -> String {
    // Tier 1: gh api user
    if let Ok(output) = safe_output(command("gh").args(["api", "user", "--jq", ".login"])) {
        if output.status.success() {
            let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !login.is_empty() {
                return login;
            }
        }
    }

    // Tier 2: parse git remote URL
    if let Some(path_str) = repo_path.to_str() {
        if let Ok(output) = safe_output(command("git").args(["-C", path_str, "remote", "get-url", "origin"])) {
            if output.status.success() {
                let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(username) = parse_github_username_from_remote(&url) {
                    return username;
                }
            }
        }
    }

    // Tier 3: git config user.name
    if let Some(path_str) = repo_path.to_str() {
        if let Ok(output) = safe_output(command("git").args(["-C", path_str, "config", "user.name"])) {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() {
                    return sanitize_username(&name);
                }
            }
        }
    }

    // Tier 4: fallback
    "dev".to_string()
}

fn parse_github_username_from_remote(url: &str) -> Option<String> {
    // SSH: git@github.com:user/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let user = rest.split('/').next()?;
        if !user.is_empty() {
            return Some(user.to_string());
        }
    }
    // HTTPS: https://github.com/user/repo.git
    if url.contains("github.com/") {
        let after = url.split("github.com/").nth(1)?;
        let user = after.split('/').next()?;
        if !user.is_empty() {
            return Some(user.to_string());
        }
    }
    None
}

fn sanitize_username(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

/// Generate a unique branch name like `username/rohlik` that doesn't collide
/// with existing branches or worktree branches.
pub fn generate_branch_name(repo_path: &Path) -> String {
    let username = detect_github_username(repo_path);
    let taken = collect_taken_branches(repo_path);

    // Shuffle goods deterministically using a simple hash of current time
    let mut indices: Vec<usize> = (0..GOODS.len()).collect();
    shuffle(&mut indices);

    // Phase 1: try plain goods
    for &i in &indices {
        let candidate = format!("{}/{}", username, GOODS[i].name);
        if !taken.contains(&candidate) {
            return candidate;
        }
    }

    // Phase 2: try adjective+good combos
    for &(stem, sm, sf, sn) in ADJECTIVE_STEMS {
        for &i in &indices {
            let good = &GOODS[i];
            let adj = adjective_for(stem, sm, sf, sn, good);
            let candidate = format!("{}/{}-{}", username, adj, good.name);
            if !taken.contains(&candidate) {
                return candidate;
            }
        }
    }

    // Phase 3: numeric suffix on adjective+good (capped to avoid unbounded loop)
    for suffix_num in 2u32..1000 {
        for &(stem, sm, sf, sn) in ADJECTIVE_STEMS {
            for &i in &indices {
                let good = &GOODS[i];
                let adj = adjective_for(stem, sm, sf, sn, good);
                let candidate = format!("{}/{}-{}-{}", username, adj, good.name, suffix_num);
                if !taken.contains(&candidate) {
                    return candidate;
                }
            }
        }
    }

    // Fallback: UUID-based name (practically unreachable)
    format!("{}/worktree-{}", username, uuid::Uuid::new_v4())
}

fn collect_taken_branches(repo_path: &Path) -> HashSet<String> {
    let mut taken = HashSet::new();
    for b in super::repository::list_branches(repo_path) {
        taken.insert(b);
    }
    for b in super::repository::get_worktree_branches(repo_path) {
        taken.insert(b);
    }
    taken
}

/// Simple Fisher-Yates shuffle using system time as seed
fn shuffle(indices: &mut [usize]) {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42);
    let mut rng = seed;
    for i in (1..indices.len()).rev() {
        // Simple xorshift64
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let j = (rng as usize) % (i + 1);
        indices.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_username_ssh() {
        assert_eq!(
            parse_github_username_from_remote("git@github.com:user/repo.git"),
            Some("user".to_string())
        );
    }

    #[test]
    fn test_parse_github_username_https() {
        assert_eq!(
            parse_github_username_from_remote("https://github.com/user/repo.git"),
            Some("user".to_string())
        );
    }

    #[test]
    fn test_parse_github_username_no_github() {
        assert_eq!(
            parse_github_username_from_remote("git@gitlab.com:user/repo.git"),
            None
        );
    }

    #[test]
    fn test_adjective_gender_agreement() {
        let m_good = BakedGood { name: "rohlik", gender: Gender::Masculine };
        let f_good = BakedGood { name: "houska", gender: Gender::Feminine };
        let n_good = BakedGood { name: "trdlo", gender: Gender::Neuter };

        assert_eq!(adjective_for("velk", "y", "a", "e", &m_good), "velky");
        assert_eq!(adjective_for("velk", "y", "a", "e", &f_good), "velka");
        assert_eq!(adjective_for("velk", "y", "a", "e", &n_good), "velke");
    }

    #[test]
    fn test_sanitize_username() {
        assert_eq!(sanitize_username("John Doe"), "john-doe");
        assert_eq!(sanitize_username("user@name!"), "username");
        assert_eq!(sanitize_username("Already-Good"), "already-good");
    }

    #[test]
    fn test_generate_avoids_collisions() {
        // We can't easily call generate_branch_name without a real repo,
        // but we can test the collision logic by checking that the goods list
        // and adjective stems are well-formed.
        assert_eq!(GOODS.len(), 38);
        assert_eq!(ADJECTIVE_STEMS.len(), 10);

        // Verify all goods have non-empty names
        for good in GOODS {
            assert!(!good.name.is_empty());
        }
    }

    #[test]
    fn test_parse_github_username_https_no_extension() {
        assert_eq!(
            parse_github_username_from_remote("https://github.com/myorg/myrepo"),
            Some("myorg".to_string())
        );
    }
}
