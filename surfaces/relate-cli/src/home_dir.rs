//! The per-user base reads every home-relative CLI path shares: the two
//! read rules (set and non-empty; set, non-empty and absolute) and the
//! home directory they resolve. The config directory (`config_dir`,
//! ADR-0545 §6) and the agent catalog's `~` expansion (`skill::agents`)
//! both read their bases through here, so the rules are spelled once.
//!
//! The home directory is `$HOME` when set, non-empty and rooted, else
//! `%USERPROFILE%` when set, non-empty and absolute. A relative `$HOME`
//! names no base: home-relative paths resolved against the working
//! directory would make every consumer cwd-dependent. Stock PowerShell and
//! `cmd` on Windows set no `$HOME`, so without the second tier every
//! home-relative path resolved to nothing there. A POSIX-style shell on
//! Windows (Git Bash, MSYS, Cygwin) sets `$HOME`, which keeps priority so
//! its existing state does not move. `std::env::home_dir` is not used:
//! it reads `%USERPROFILE%` before `$HOME` on Windows, and it cannot take
//! an injected environment for tests.
//!
//! The reads take an `Option<String>` environment: a variable that is
//! unset, empty, or not valid UTF-8 names no base, and resolution falls
//! through to the next tier. That differs on purpose from
//! `release_channel::env_nonempty`, which refuses a non-UTF-8 value: a
//! release seam must never silently fall back to its default, while a home
//! or config base with another tier to try degrades to that tier.

use std::path::PathBuf;

/// One base read's shared half: set and non-empty. An empty value never
/// names a base — it is how shells and CI runners spell "cleared".
fn non_empty(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    env(key).filter(|v| !v.is_empty())
}

/// The absolute-reading tiers' shared read: set, non-empty, and absolute.
/// A relative value never names a base, because a base resolved against
/// the current working directory would make every consumer cwd-dependent.
/// A Windows host's native `%APPDATA%` / `%USERPROFILE%` values are
/// absolute by construction. `$HOME` reads through [`home_base`] instead,
/// which asks only for a root, so the POSIX-shell spellings (`/c/Users/…`)
/// a Windows host cannot classify as absolute still resolve.
pub(crate) fn absolute_base(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<PathBuf> {
    non_empty(env, key)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// The `$HOME` tier read through `env`: set, non-empty, and rooted. Rooted,
/// not absolute: Git Bash, MSYS and Cygwin hand native children POSIX
/// spellings such as `/c/Users/u`, which have a root but no drive on
/// Windows. A relative value names no base.
pub(crate) fn home_base(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    non_empty(env, "HOME")
        .map(PathBuf::from)
        .filter(|path| path.has_root())
}

/// The home directory read through `env`: a rooted `$HOME`, else an
/// absolute `%USERPROFILE%`. `None` when neither resolves.
#[cfg_attr(not(any(feature = "skill", feature = "upgrade")), allow(dead_code))]
pub(crate) fn home_dir_with(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    home_base(env).or_else(|| absolute_base(env, "USERPROFILE"))
}

/// The XDG config base read through `env`: an absolute `$XDG_CONFIG_HOME`,
/// else `<home>/.config`. A relative `$XDG_CONFIG_HOME` is treated as unset,
/// as the XDG spec requires and as the config directory's own first tier
/// reads it. `None` when neither resolves.
#[cfg_attr(not(any(feature = "skill", feature = "upgrade")), allow(dead_code))]
pub(crate) fn xdg_config_base_with(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    absolute_base(env, "XDG_CONFIG_HOME")
        .or_else(|| home_dir_with(env).map(|home| home.join(".config")))
}

/// A base value `is_absolute` accepts on the host running the test:
/// Windows wants a drive prefix, everything else a leading slash. The one
/// host-absolute fixture every base-resolution test (here, `config_dir`,
/// `skill::agents`) builds its values from.
#[cfg(test)]
pub(crate) fn host_absolute(rest: &str) -> String {
    if cfg!(windows) {
        format!("C:\\{rest}")
    } else {
        format!("/{rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::env_of;

    #[test]
    fn home_wins_over_userprofile() {
        let profile = host_absolute("Users/u");
        let pairs = [("HOME", "/home/u"), ("USERPROFILE", profile.as_str())];
        assert_eq!(
            home_dir_with(&env_of(&pairs)),
            Some(PathBuf::from("/home/u"))
        );
    }

    #[test]
    fn userprofile_resolves_when_home_is_unset_or_empty() {
        // Stock PowerShell and cmd on Windows set no HOME.
        let profile = host_absolute("Users/u");
        assert_eq!(
            home_dir_with(&env_of(&[("USERPROFILE", profile.as_str())])),
            Some(PathBuf::from(&profile))
        );
        assert_eq!(
            home_dir_with(&env_of(&[("HOME", ""), ("USERPROFILE", profile.as_str())])),
            Some(PathBuf::from(&profile))
        );
    }

    #[test]
    fn a_relative_or_empty_userprofile_never_resolves() {
        assert_eq!(
            home_dir_with(&env_of(&[("USERPROFILE", "relprofile")])),
            None
        );
        assert_eq!(home_dir_with(&env_of(&[("USERPROFILE", "")])), None);
        assert_eq!(home_dir_with(&env_of(&[])), None);
    }

    #[test]
    fn xdg_config_base_ignores_a_relative_value() {
        let profile = host_absolute("Users/u");
        let pairs = [
            ("XDG_CONFIG_HOME", "relconf"),
            ("USERPROFILE", profile.as_str()),
        ];
        assert_eq!(
            xdg_config_base_with(&env_of(&pairs)),
            Some(PathBuf::from(&profile).join(".config"))
        );
        let absolute = host_absolute("xdg");
        let pairs = [("XDG_CONFIG_HOME", absolute.as_str()), ("HOME", "/home/u")];
        assert_eq!(
            xdg_config_base_with(&env_of(&pairs)),
            Some(PathBuf::from(&absolute))
        );
        assert_eq!(xdg_config_base_with(&env_of(&[])), None);
    }

    #[test]
    fn a_relative_home_never_resolves() {
        // A relative HOME would make every home-relative path depend on the
        // working directory; the USERPROFILE tier decides instead.
        let profile = host_absolute("Users/u");
        let pairs = [("HOME", "relhome"), ("USERPROFILE", profile.as_str())];
        assert_eq!(
            home_dir_with(&env_of(&pairs)),
            Some(PathBuf::from(&profile))
        );
        assert_eq!(home_dir_with(&env_of(&[("HOME", "relhome")])), None);
    }

    #[test]
    fn home_keeps_any_rooted_spelling() {
        // Git Bash hands native children a POSIX-style HOME on some setups:
        // rooted, though not drive-absolute on Windows, so it resolves.
        assert_eq!(
            home_dir_with(&env_of(&[("HOME", "/c/Users/u")])),
            Some(PathBuf::from("/c/Users/u"))
        );
    }
}
