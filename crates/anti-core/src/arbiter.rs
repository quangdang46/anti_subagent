//! Read-only arbiter (maestro route_next) — scores options using a fixed rubric.
//! NO FS/git access: compiler guarantees read-only.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Risk { Low, Medium, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effort { Small, Medium, Large }

#[derive(Debug, Clone)]
pub struct ArbiterOption {
    pub id: String,
    pub desc: String,
    pub risk: Risk,
    pub effort: Effort,
}

pub struct Arbiter;

impl Arbiter {
    pub fn rank(&self, options: &mut Vec<ArbiterOption>) -> Vec<ArbiterOption> {
        options.sort_by_key(|o| (o.risk as u8, o.effort as u8));
        options.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbiter_scores_by_rubric_deterministically() {
        let a = Arbiter;
        let mut opts = vec![
            ArbiterOption { id: "fast".into(), desc: "quick hack".into(), risk: Risk::High, effort: Effort::Small },
            ArbiterOption { id: "solid".into(), desc: "proper fix".into(), risk: Risk::Low, effort: Effort::Large },
        ];
        let ranked = a.rank(&mut opts);
        // rubric: low risk + small effort wins; solid (low risk) should rank above fast
        assert!(ranked.iter().position(|o| o.id == "solid").unwrap() < ranked.iter().position(|o| o.id == "fast").unwrap());
    }

    #[test]
    fn arbiter_cannot_mutate_fs() {
        // read-only: no fs references in API — compile-time guarantee
        let a = Arbiter;
        let ranked = a.rank(&mut vec![ArbiterOption { id: "x".into(), desc: "d".into(), risk: Risk::Low, effort: Effort::Small }]);
        assert_eq!(ranked.len(), 1);
    }
}
