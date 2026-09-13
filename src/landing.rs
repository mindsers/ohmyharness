//! Whether a branch's work is already on trunk, and how omh can tell.
//!
//! `rm` used to ask one question — `rev-list --count {base}..{branch}` — and
//! that question is about **ancestry**. A squash merge, which is this project's
//! own merge button, writes a new commit with a new sha and different parents,
//! so a branch whose content landed a month ago still answers *two commits*.
//! Measured here: `omh/s02` held two commits squash-merged as `58acbaa`, and
//! `omh s02 rm` reported `2 commits to review` and kept the branch for a month.
//!
//! So the answer is not a number. It is one of five states, and two of them —
//! *omh looked and found nothing* and *omh could not look that far* — are the
//! pair a count cannot tell apart. A branch is deleted on the strength of this,
//! which is why the reason travels inside the answer rather than beside it.
//!
//! **The decision is here and the git is not.** `settle` and
//! `settle_with_change` are pure functions over values; `History` is the seam
//! the facts arrive through, implemented by `session::GitHistory` for real work
//! and by `FakeHistory` below for the one thing no git fixture produces
//! reliably — a read that fails. (A look that stops at its limit *is*
//! fixturable, and `a_survey_that_stopped_at_its_limit_says_so_and_still_counts`
//! does it against real git.) The fake pins the *policy*. Only the tests
//! against real git pin the plumbing, and nothing here should be cited as
//! evidence that a git invocation is right.

use anyhow::Result;

/// How far the look at trunk got.
///
/// `Capped` is not a smaller `Whole`. A proof found inside the window is still
/// a proof — evidence does not stop being evidence because omh stopped reading
/// — but a look that stopped early may never answer `Unreviewed`, which claims
/// omh looked and found nothing. That narrower rule is enforced by `settle`
/// matching on this rather than by anybody remembering it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    Whole,
    Capped(usize),
}

/// What proves trunk already holds this branch's work, and where.
///
/// Both carry the commit it landed as, because a proof nobody can look up is a
/// claim. They are kept apart because they answer differently sized questions:
/// `SameTree` says trunk has a commit with this branch's exact tree, and
/// `SamePatch` says trunk has a commit whose change is this branch's whole
/// change — the shape a squash of several commits takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    SameTree { at: String },
    SamePatch { at: String },
}

impl Proof {
    pub fn at(&self) -> &str {
        match self {
            Self::SameTree { at } | Self::SamePatch { at } => at,
        }
    }
}

/// What keeping this branch would preserve.
///
/// The question `remove` needs answered, and the one a `usize` could not carry:
/// `Ok(0)` and `Err(_)` were the only two ways to say *nothing to keep* and
/// *omh could not tell*, and one of them is a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holding {
    /// A scratch session (`omh auth`, `omh doctor`), or an id nothing created.
    NoBranch,
    /// Trunk already contains these commits, by ancestry. Nothing to keep.
    Nothing,
    /// Trunk holds this content under a different sha, and here is the proof.
    Landed { commits: usize, by: Proof },
    /// omh read the whole window and found no proof — the branch holds work
    /// nobody has reviewed. Only a `Reach::Whole` look can say this.
    Unreviewed { commits: usize },
    /// omh could not tell.
    ///
    /// `commits` is `Some` when the count was taken and only the *landing* is
    /// unknown, `None` when nothing resolved at all — a known count rendered as
    /// uncountable is a second wrong answer on top of the first.
    Unsettled { commits: Option<usize>, why: String },
}

/// What the facts in hand are enough to say.
///
/// The expensive probe is **asked for by the decision**, not chosen by the
/// caller. `omh s` renders a row per session and cannot afford it (measured:
/// 235 ms on a range of 84 commits, against 9 ms for the look it already
/// pays for), so it declines the request — and declining is not an answer, it
/// is the absence of one. Were the caller choosing, the cheap path would
/// quietly become a claim that nothing landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settling {
    Settled(Holding),
    NeedsPatchProof { commits: usize },
}

/// How a session stands against trunk: commits trunk has that it does not
/// (`behind`), and commits it has that trunk does not (`ahead`). Named so the
/// two counts are not swappable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub behind: usize,
    pub ahead: usize,
}

/// One look at how a branch sits against trunk, and the cheap evidence.
///
/// Everything here comes out of a single `git log`, which is the same process
/// the dashboard already spends to print *N behind main* — the tree evidence
/// rides along rather than costing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    pub standing: Standing,
    /// The branch tip's tree, or `None` when there is no branch to have one.
    pub tip: Option<String>,
    /// Trunk's commits since the fork, **oldest first**, truncated to the
    /// limit the survey was asked for — so this list is what omh *looked at*,
    /// and `reach` says whether that was all of them.
    pub trunk: Vec<SeenTree>,
    pub reach: Reach,
}

/// One trunk commit and the tree it left behind.
///
/// A struct rather than a pair: both halves are 40 hex characters, they are
/// compared and reported one field apart, and reporting a tree as the commit
/// the work landed as would be a proof nobody can look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenTree {
    pub tree: String,
    pub at: String,
}

/// Where the facts come from, so the decision can be tested without a repo.
///
/// The same shape as `cmd::session::Entry`, and for the same reason: the states
/// worth testing are the ones a fixture cannot produce.
pub trait History {
    fn exists(&self, branch: &str) -> Result<bool>;
    fn survey(&self, base: &str, branch: &str, limit: usize) -> Result<Survey>;
    /// Trunk's commits since the fork whose change **is** this branch's whole
    /// change, oldest first.
    ///
    /// Byte-for-byte, not by patch id — see `GitHistory::same_change` for why
    /// an id can only nominate a candidate.
    fn same_change(&self, base: &str, branch: &str) -> Result<Vec<String>>;
}

/// How far back along trunk omh looks for a squash.
pub const SQUASH_SCAN_LIMIT: usize = 500;

/// What the cheap evidence settles, and what it cannot.
///
/// Order matters twice. Ancestry first, because a branch trunk already contains
/// needs no proof of anything. Then the tree, **oldest match first**: a revert
/// and a re-apply leave two trunk commits with one tree, and the older is the
/// one the work landed in — the newer merely restored it, and sending a reader
/// there sends them to the wrong pull request.
pub fn settle(survey: &Survey) -> Settling {
    let commits = survey.standing.ahead;
    if commits == 0 {
        return Settling::Settled(Holding::Nothing);
    }
    if let Some(tip) = &survey.tip {
        if let Some(seen) = survey.trunk.iter().find(|seen| &seen.tree == tip) {
            return Settling::Settled(Holding::Landed {
                commits,
                by: Proof::SameTree {
                    at: seen.at.clone(),
                },
            });
        }
    }
    match &survey.reach {
        // Not `Unreviewed`, and not a patch probe either: asking for more
        // evidence about a range omh never read is work that cannot produce an
        // answer, and reporting the branch as unreviewed is the collapse this
        // whole module exists to refuse.
        Reach::Capped(limit) => Settling::Settled(Holding::Unsettled {
            commits: Some(commits),
            why: format!(
                "omh looked at the {limit} commits trunk gained after this branch forked \
                 and did not find this work; trunk has {behind} since then, so it may have \
                 landed past where omh looked",
                behind = survey.standing.behind
            ),
        }),
        Reach::Whole => Settling::NeedsPatchProof { commits },
    }
}

/// The expensive evidence, once the cheap look has asked for it.
///
/// `same` is trunk's commits whose change is this branch's whole change —
/// the shape a squash takes: several commits arriving as one, so no individual
/// patch id matches and `git cherry` reports nothing. The first is the oldest,
/// for the revert-and-reapply reason `settle` gives.
///
/// No evidence is `Unreviewed`, never `Landed`. Two absences comparing equal
/// is how a branch gets deleted on the strength of neither side having said
/// anything, so an empty name is not a match either.
pub fn settle_with_change(commits: usize, same: &[String]) -> Holding {
    match same.iter().find(|at| !at.is_empty()) {
        Some(at) => Holding::Landed {
            commits,
            by: Proof::SamePatch { at: at.clone() },
        },
        None => Holding::Unreviewed { commits },
    }
}

/// The whole decision, over a `History`.
///
/// Every failure becomes `Unsettled` **here**, at the boundary, so no caller
/// downstream can `?` one into an absence or `.ok()` it into a `None` that
/// reads as a clean answer. That collapse is the one `doctor`'s leftovers row
/// shipped — `.flatten()`, `.ok()`, one `Option` for several reasons, a
/// `Result` for a whole sweep — and it is a deletion this time, not a report.
pub fn holding(history: &dyn History, branch: Option<&str>, base: &str, limit: usize) -> Holding {
    let Some(branch) = branch else {
        return Holding::NoBranch;
    };
    // Asked of a branch that exists, and that is not a formality: every read
    // below fails for a missing branch exactly as it does for a missing trunk,
    // so an id nothing ever created answered *could not tell* and was reported
    // as a branch kept — over a branch that was never there.
    match history.exists(branch) {
        Err(e) => {
            return Holding::Unsettled {
                commits: None,
                why: format!("{e:#}"),
            }
        }
        Ok(false) => return Holding::NoBranch,
        Ok(true) => {}
    }
    let survey = match history.survey(base, branch, limit) {
        Ok(survey) => survey,
        Err(e) => {
            return Holding::Unsettled {
                commits: None,
                why: format!("{e:#}"),
            }
        }
    };
    match settle(&survey) {
        Settling::Settled(holding) => holding,
        // The count is known here and only the landing is not, which is why it
        // travels into `Unsettled` rather than being dropped for a `None` the
        // caller would render as *omh could not count it*.
        Settling::NeedsPatchProof { commits } => match history.same_change(base, branch) {
            Ok(same) => settle_with_change(commits, &same),
            Err(e) => Holding::Unsettled {
                commits: Some(commits),
                why: format!("{e:#}"),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survey(
        ahead: usize,
        behind: usize,
        tip: &str,
        trunk: &[(&str, &str)],
        reach: Reach,
    ) -> Survey {
        Survey {
            standing: Standing { behind, ahead },
            tip: Some(tip.into()),
            trunk: trunk
                .iter()
                .map(|(tree, at)| SeenTree {
                    tree: (*tree).to_string(),
                    at: (*at).to_string(),
                })
                .collect(),
            reach,
        }
    }

    /// Nothing on the branch is nothing to keep, and that has not changed.
    #[test]
    fn a_branch_trunk_already_contains_holds_nothing() {
        let s = survey(0, 3, "tree-a", &[("tree-a", "c1")], Reach::Whole);
        assert_eq!(settle(&s), Settling::Settled(Holding::Nothing));
    }

    /// The tree evidence, which is what a rebased-then-squashed branch leaves.
    #[test]
    fn a_trunk_commit_with_the_branchs_tree_is_the_proof() {
        let s = survey(
            2,
            84,
            "tree-x",
            &[("tree-w", "c1"), ("tree-x", "c2")],
            Reach::Whole,
        );
        assert_eq!(
            settle(&s),
            Settling::Settled(Holding::Landed {
                commits: 2,
                by: Proof::SameTree { at: "c2".into() }
            })
        );
    }

    /// Oldest first, because the older commit is the one that landed it.
    ///
    /// A revert and a re-apply leave two commits on trunk with the same tree.
    /// Naming the newer one tells a reader the work landed in a commit that
    /// merely restored it, which sends them to the wrong pull request.
    #[test]
    fn the_older_of_two_matching_trunk_commits_is_the_one_named() {
        let s = survey(
            1,
            3,
            "tree-x",
            &[("tree-x", "older"), ("tree-q", "c2"), ("tree-x", "newer")],
            Reach::Whole,
        );
        match settle(&s) {
            Settling::Settled(Holding::Landed { by, .. }) => assert_eq!(by.at(), "older"),
            other => panic!("the tree is on trunk twice and omh must name the first: {other:?}"),
        }
    }

    /// A look that stopped early cannot answer, and must not pretend to.
    ///
    /// This is the whole reason `Reach` exists as a value rather than a
    /// comment: `Unreviewed` from a capped look is *omh did not look there*
    /// rendered as *there is nothing there*, and the caller deletes on it.
    #[test]
    fn a_look_that_hit_its_limit_is_a_question_not_an_answer() {
        let s = survey(1, 900, "tree-x", &[("tree-w", "c1")], Reach::Capped(500));
        match settle(&s) {
            Settling::Settled(Holding::Unsettled { commits, why }) => {
                assert_eq!(
                    commits,
                    Some(1),
                    "the count was taken; only the landing is unknown"
                );
                assert!(
                    why.contains("500"),
                    "and it says how far omh looked, so the answer is actionable: {why}"
                );
            }
            other => panic!("a capped look may not settle anything: {other:?}"),
        }
    }

    /// The expensive probe is requested, and only when it could still help.
    #[test]
    fn the_patch_is_asked_for_only_when_a_whole_look_found_nothing() {
        let whole = survey(1, 3, "tree-x", &[("tree-w", "c1")], Reach::Whole);
        assert_eq!(settle(&whole), Settling::NeedsPatchProof { commits: 1 });

        let capped = survey(1, 900, "tree-x", &[("tree-w", "c1")], Reach::Capped(500));
        assert!(
            !matches!(settle(&capped), Settling::NeedsPatchProof { .. }),
            "asking for more evidence about a range omh did not read is work for no answer"
        );
    }

    /// The squash of several commits: no tree matches, and trunk holds a
    /// commit whose change is this branch's whole change.
    #[test]
    fn the_branchs_whole_change_matching_one_trunk_commit_is_the_proof() {
        assert_eq!(
            settle_with_change(2, &["c2".to_string()]),
            Holding::Landed {
                commits: 2,
                by: Proof::SamePatch { at: "c2".into() }
            }
        );
    }

    /// Two commits with the branch's change: the older is the one named, for
    /// the same reason the tree probe names the older.
    #[test]
    fn the_older_of_two_matching_changes_is_the_one_named() {
        match settle_with_change(1, &["older".to_string(), "newer".to_string()]) {
            Holding::Landed { by, .. } => assert_eq!(by.at(), "older"),
            other => panic!("trunk holds this change twice: {other:?}"),
        }
    }

    /// Nothing to compare is not a match with everything.
    ///
    /// A branch whose change is empty (an `--allow-empty` commit) matches no
    /// trunk commit, and a name that is empty is not a commit anybody can look
    /// up — read as values, two absences would compare equal and delete a
    /// branch on the strength of neither side having said anything.
    #[test]
    fn no_change_in_common_proves_nothing() {
        assert_eq!(
            settle_with_change(1, &[]),
            Holding::Unreviewed { commits: 1 }
        );
        assert_eq!(
            settle_with_change(1, &[String::new()]),
            Holding::Unreviewed { commits: 1 }
        );
    }

    /// Looked, found nothing, says so — the one state that keeps the branch
    /// without hedging.
    #[test]
    fn a_whole_look_with_no_match_is_unreviewed() {
        assert_eq!(
            settle_with_change(3, &[]),
            Holding::Unreviewed { commits: 3 }
        );
    }

    /// A history that answers errors, which no git fixture can be made to do
    /// reliably — and every one of them has to keep the branch.
    struct FakeHistory {
        exists: Result<bool>,
        survey: Result<Survey>,
        same: Result<Vec<String>>,
    }

    impl Default for FakeHistory {
        fn default() -> Self {
            Self {
                exists: Ok(true),
                survey: Ok(survey(1, 3, "tree-x", &[("tree-w", "c1")], Reach::Whole)),
                same: Ok(vec![]),
            }
        }
    }

    impl History for FakeHistory {
        fn exists(&self, _branch: &str) -> Result<bool> {
            match &self.exists {
                Ok(b) => Ok(*b),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
        fn survey(&self, _base: &str, _branch: &str, _limit: usize) -> Result<Survey> {
            match &self.survey {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
        fn same_change(&self, _base: &str, _branch: &str) -> Result<Vec<String>> {
            match &self.same {
                Ok(same) => Ok(same.clone()),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    #[test]
    fn a_scratch_session_has_no_branch_to_hold_anything() {
        let h = FakeHistory::default();
        assert_eq!(holding(&h, None, "main", 500), Holding::NoBranch);
    }

    #[test]
    fn a_branch_that_is_not_there_is_not_a_branch_omh_could_not_read() {
        let h = FakeHistory {
            exists: Ok(false),
            ..Default::default()
        };
        assert_eq!(holding(&h, Some("omh/s01"), "main", 500), Holding::NoBranch);
    }

    /// Every read that fails keeps the branch and carries git's own words.
    #[test]
    fn a_read_omh_could_not_make_is_never_a_count_of_none() {
        for (h, commits, needle) in [
            (
                FakeHistory {
                    exists: Err(anyhow::anyhow!("not a git repository")),
                    ..Default::default()
                },
                None,
                "not a git repository",
            ),
            (
                FakeHistory {
                    survey: Err(anyhow::anyhow!("unknown revision main")),
                    ..Default::default()
                },
                None,
                "unknown revision",
            ),
            (
                FakeHistory {
                    same: Err(anyhow::anyhow!("bad object")),
                    ..Default::default()
                },
                Some(1),
                "bad object",
            ),
        ] {
            match holding(&h, Some("omh/s01"), "main", 500) {
                Holding::Unsettled { commits: c, why } => {
                    assert_eq!(c, commits, "what omh did count survives what it could not");
                    assert!(
                        why.contains(needle),
                        "git's own words reach the caller: {why}"
                    );
                }
                other => panic!("a failed read may not settle anything: {other:?}"),
            }
        }
    }
}
