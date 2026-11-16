//! Root Rune view assembling header, Golden Line canvas, Pro drawer, and modal overlays.

use rune::{Cx, El, col, el};

use crate::model::AutoTuneStep;
use crate::state::AppState;
use crate::theme::{TineTheme, spacing};

/// Renders the complete Tine UI tree.
#[must_use]
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = TineTheme::dark();
    let wizard_active = !matches!(s.model.autotune.step, AutoTuneStep::Idle);
    let onboarding_active = s.model.first_run && !s.ui.onboarding_dismissed;
    let modal_active = wizard_active || onboarding_active;

    let mut main = col()
        .key("main-shell")
        .w_full()
        .h_full()
        .fill(t.bg)
        .px(spacing::LG)
        .py(spacing::MD)
        .gap(spacing::MD)
        .child(crate::views::header::view(s, t))
        .child(crate::views::canvas::view(s, cx))
        .child(crate::views::pro_rack::view(s, cx, t));

    if modal_active {
        main = main.inert();
    }

    let mut root = el().key("root").w_full().h_full().fill(t.bg).child(main);

    // The wizard takes precedence; onboarding resumes behind it when dismissed.
    if wizard_active {
        root = root.child(crate::views::autotune::view(s, t));
    } else if onboarding_active {
        root = root.child(crate::views::onboarding::view(s, t));
    }

    root
}
