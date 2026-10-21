//! Ported from tests/test_sc_audit_survey.py.

use admiralty_audit::screen::screen_state;

/// Ported from tests/test_sc_audit_survey.py::SurveyRegex::test_priority_label_not_survey
#[test]
fn test_priority_label_not_survey() {
    let t = "  NOW. P1 = JopeDime GPU (all 3 GPUs at 0%) + Admiralty; re-drive idle screens;\nesc to cancel";
    assert!(!screen_state(t).survey);
}

/// Ported from tests/test_sc_audit_survey.py::SurveyRegex::test_real_rating_scale_is_survey
#[test]
fn test_real_rating_scale_is_survey() {
    assert!(
        screen_state("How is Claude doing this session?\n 1: Bad   2: Fine   3: Good   0: Dismiss")
            .survey
    );
    assert!(screen_state("Rate this session  1 = poor  2 = ok  3 = great").survey);
}

/// Ported from tests/test_sc_audit_survey.py::SurveyRegex::test_plain_assignment_not_survey
#[test]
fn test_plain_assignment_not_survey() {
    assert!(!screen_state("x1 = 5; y2 = 6\nP2 = Admiralty").survey);
}
