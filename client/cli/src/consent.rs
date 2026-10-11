//! What every sign-up asks before an account is made: month and year of
//! birth, a parent's or guardian's agreement for 13 to 17, and agreement to
//! the Terms of use and the Privacy policy. Only the check happens, here:
//! the birth date is never kept, sent or logged. The same rule as the
//! sign-up pages (servers/web-shared/age.js).

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use anyhow::{Result, bail};
use auth::KeyStore;

/// kept once someone under 13 answered on this machine, so answering again
/// changes nothing; it holds that alone, never the answer
const REFUSED: &str = "age-refused";
const UNDER_13: &str = "Commonty is only for people aged 13 or over.";

#[derive(clap::Args)]
pub struct Consent {
    /// Your month and year of birth: only checked here, never kept or sent.
    /// Asked for when it is not given and this is a terminal.
    #[arg(long, value_name = "YYYY-MM")]
    born: Option<String>,
    /// You are 13 to 17, and a parent or guardian agrees to you using Commonty.
    #[arg(long)]
    guardian_agrees: bool,
    /// You agree to the Terms of use and the Privacy policy (/docs/policies/terms
    /// and /docs/policies/privacy on the site).
    #[arg(long)]
    agree_terms: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Age {
    Under13,
    Teen,
    Adult,
}

/// Whole months from a birth month to now's, the birth month counted as
/// reached: no day is asked, so someone whose birthday is this month is
/// taken to have had it.
pub fn age(born: (i64, u32), now: (i64, u32)) -> Age {
    let months = (now.0 - born.0) * 12 + i64::from(now.1) - i64::from(born.1);
    if months < 13 * 12 {
        Age::Under13
    } else if months < 18 * 12 {
        Age::Teen
    } else {
        Age::Adult
    }
}

fn parse(s: &str) -> Option<(i64, u32)> {
    let (y, m) = s.trim().split_once('-')?;
    let y: i64 = y.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    ((1..=12).contains(&m) && (1900..=9999).contains(&y)).then_some((y, m))
}

/// this month, in UTC, from the clock
fn this_month() -> (i64, u32) {
    let days = (identity::now() / 86400) as i64;
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m)
}

fn ask(question: &str) -> Result<String> {
    print!("{question} ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn yes(question: &str) -> Result<bool> {
    Ok(matches!(
        ask(&format!("{question} [y/N]"))?.to_lowercase().as_str(),
        "y" | "yes"
    ))
}

impl Consent {
    /// The questions, from the flags or asked at the terminal. The version
    /// of the terms agreed to, for the entry (identity::TERMS).
    pub fn check(&self, keys: &impl KeyStore, site: &str) -> Result<&'static str> {
        if keys.get(REFUSED)?.is_some() {
            bail!(UNDER_13);
        }
        let terminal = std::io::stdin().is_terminal();
        let asked =
            |flag: &str| anyhow::anyhow!("{flag} is needed when this is not run at a terminal");
        let born = match &self.born {
            Some(b) => b.clone(),
            None if terminal => ask("Your month and year of birth (YYYY-MM):")?,
            None => return Err(asked("--born")),
        };
        let Some(born) = parse(&born) else {
            bail!("a month and year of birth looks like 2001-04");
        };
        match age(born, this_month()) {
            Age::Under13 => {
                keys.set(REFUSED, "1")?;
                bail!(UNDER_13);
            }
            Age::Teen => {
                let ok = self.guardian_agrees
                    || (terminal && yes("A parent or guardian agrees to you using Commonty?")?);
                if !ok {
                    bail!("under 18, a parent or guardian has to agree to you using Commonty");
                }
            }
            Age::Adult => {}
        }
        let agreed = self.agree_terms
            || (terminal
                && yes(&format!(
                    "Do you agree to the Terms of use ({site}/docs/policies/terms) and the \
                     Privacy policy ({site}/docs/policies/privacy)?"
                ))?);
        if !agreed {
            if !terminal {
                return Err(asked("--agree-terms"));
            }
            bail!("an account needs your agreement to the Terms of use and the Privacy policy");
        }
        Ok(identity::TERMS)
    }
}

/// the site a directory is on, for the links to the terms
pub fn site_of(directories: &[String]) -> String {
    let host = directories
        .first()
        .and_then(|d| d.split("://").nth(1))
        .and_then(|h| h.split(['/', ':']).next())
        .unwrap_or("commonty.org");
    format!("https://{}", host.strip_prefix("home.").unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_birth_month_counts_as_reached() {
        let now = (2026, 10);
        assert_eq!(age((2013, 11), now), Age::Under13);
        assert_eq!(age((2013, 10), now), Age::Teen);
        assert_eq!(age((2008, 11), now), Age::Teen);
        assert_eq!(age((2008, 10), now), Age::Adult);
        assert_eq!(age((1950, 1), now), Age::Adult);
    }

    #[test]
    fn a_month_and_year_and_nothing_else() {
        assert_eq!(parse("2001-04"), Some((2001, 4)));
        assert_eq!(parse(" 1999-12 "), Some((1999, 12)));
        for bad in ["2001-13", "2001-00", "2001", "04-2001x", "2001-04-02", ""] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn this_month_is_a_month() {
        let (y, m) = this_month();
        assert!(y >= 2026 && (1..=12).contains(&m));
    }
}
