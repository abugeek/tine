//! Root Rune view assembling header, Golden Line canvas, Pro drawer, and modal overlays.

use rune::{Cx, El, col, el, row, text};

use crate::model::AutoTuneStep;
use crate::state::AppState;
use crate::theme::{TineTheme, radii, spacing, typography};

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
        .child(crate::views::header::view(s, t));

    if let Some(msg) = s.model.status_banner.as_deref() {
        main = main.child(status_banner(msg, t));
    }

    main = main
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

fn status_banner(msg: &str, t: TineTheme) -> El {
    row()
        .key("status-banner")
        .role("alert")
        .label(msg.to_owned())
        .w_full()
        .px(spacing::MD)
        .py(spacing::SM)
        .gap(spacing::SM)
        .center()
        .radius(radii::MD)
        .fill(t.surface_raised)
        .border(1.0, t.danger)
        .child(el().key("status-banner/dot").size(8.0, 8.0).radius(4.0).fill(t.danger))
        .child(
            row().key("status-banner/message-wrap").grow(1.0).child(
                text(msg.to_owned())
                    .key("status-banner/message")
                    .font(typography::LABEL)
                    .color(t.text),
            ),
        )
        .child(
            row()
                .key("status-banner/dismiss")
                .role("button")
                .label("Dismiss status banner")
                .focusable(true)
                .h(26.0)
                .px(spacing::SM)
                .center()
                .radius(radii::SM)
                .fill(t.surface)
                .border(1.0, t.border)
                .on_press(|s: &mut AppState| {
                    s.model.status_banner = None;
                })
                .child(text("Dismiss").font(typography::LABEL).color(t.text)),
        )
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use super::*;
    use crate::scenes::make_golden_line;
    use crate::theme::contrast_ratio;

    #[test]
    fn status_banner_renders_and_dismisses_cleanly() {
        let t = TineTheme::dark();
        assert!(contrast_ratio(t.text, t.surface_raised) >= 4.5);
        assert!(contrast_ratio(t.text, t.surface) >= 4.5);

        // Absent when status_banner is None.
        let none_state = make_golden_line();
        assert!(none_state.model.status_banner.is_none());
        let mut rt_none = Runtime::new(
            none_state,
            view,
            (1280.0, 800.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt_none.frame(0.0);
        assert!(
            rt_none
                .elements()
                .iter()
                .all(|e| e.key.as_deref() != Some("status-banner"))
        );

        // Present when status_banner is Some(...), accessible, and dismissible.
        let mut state = make_golden_line();
        state.model.status_banner = Some(String::from("Virtual cable disconnected"));
        let mut rt = Runtime::new(
            state,
            view,
            (960.0, 640.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let elements = rt.elements();
        let banner = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("status-banner"))
            .expect("status-banner should be rendered");
        assert_eq!(banner.role, "alert");
        assert_eq!(banner.label.as_deref(), Some("Virtual cable disconnected"));

        let dismiss = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("status-banner/dismiss"))
            .expect("status-banner/dismiss should be rendered");
        assert!(dismiss.focusable);
        assert_eq!(dismiss.role, "button");
        assert_eq!(dismiss.label.as_deref(), Some("Dismiss status banner"));

        // Clicking Dismiss clears status_banner locally and removes the banner.
        let (dx, dy) = rt.find("status-banner/dismiss").expect("dismiss coordinates");
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
        rt.frame(1.0 / 60.0);

        assert!(rt.state.model.status_banner.is_none());
        assert!(rt.find("status-banner").is_none());
        assert!(rt.find("status-banner/dismiss").is_none());
    }
}
