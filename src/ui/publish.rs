use adw::prelude::*;
use adw::{ActionRow, Banner, EntryRow, NavigationPage, PreferencesGroup, Toast, ToastOverlay};
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PolicyType, ScrolledWindow, Spinner,
    TextView, WrapMode,
};

use crate::config::{self, FALLBACK_COMMIT_TEMPLATE};
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::MainShell;
use crate::workflow::aur_git;
use crate::workflow::build as build_wf;
use crate::workflow::sync;

pub fn build(shell: &MainShell, state: &AppStateRef) -> NavigationPage {
    let pkg = state.borrow().package().clone();

    let toasts = ToastOverlay::new();
    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(14)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = Label::builder()
        .label(format!("Publish — {}", pkg.title))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let sub = Label::builder()
        .label(format!(
            "Prepare clones (or reuses) the AUR Git repository at {}. For a brand-new \
             pkgbase, Git may warn that you cloned an empty repository—that is expected \
             until the server accepts your first push (see the Arch wiki AUR submission \
             guidelines).\n\n\
             Prepare stages PKGBUILD and .SRCINFO from your Sync directory into that \
             clone, shows the diff, then you can commit and push.",
            pkg.aur_ssh_url()
        ))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&sub);

    let publication_expectations = PreferencesGroup::builder()
        .title("Publication expectations")
        .description(
            "The AUR is not moderated before publication: when your push succeeds, \
             the updated sources are public. Asking for review on the Arch forums or \
             mailing lists is encouraged when you are unsure, but it is voluntary and \
             does not block or replace your own checks before you push.",
        )
        .build();
    content.append(&publication_expectations);

    // SSH must be verified before we can git-clone/push over ssh://aur@…
    let ssh_ready = state.borrow().ssh_ok;
    if !ssh_ready {
        let banner = Banner::builder()
            .title(
                "SSH is not verified yet. You can edit the PKGBUILD, build locally, and \
                 regenerate .SRCINFO, but committing and pushing is disabled.",
            )
            .button_label("Set up SSH")
            .revealed(true)
            .build();
        let nav_cb = shell.nav();
        let shell_cb = shell.clone();
        let state_cb = state.clone();
        banner.connect_button_clicked(move |_| {
            let page = ui::ssh_setup::build(
                &nav_cb,
                &shell_cb,
                &state_cb,
                ui::ssh_setup::SshSetupFlavor::FromConnection,
            );
            nav_cb.push(&page);
        });
        content.append(&banner);
    }

    let msg_group = PreferencesGroup::builder()
        .title("Commit message")
        .description(
            "Pre-filled from your default template (use {pkg} to insert the package \
             name). Edit here to change just this commit, or press “Save as default” \
             to update the template for future commits.",
        )
        .build();
    let message_row = EntryRow::builder().title("Commit message").build();

    let initial_template = state
        .borrow()
        .config
        .default_commit_message
        .clone()
        .unwrap_or_else(|| FALLBACK_COMMIT_TEMPLATE.to_string());
    message_row.set_text(&config::render_commit_template(&initial_template, &pkg.id));

    let default_text = describe_default(&state.borrow().config.default_commit_message);
    let default_hint = ActionRow::builder()
        .title("Default template")
        .subtitle(default_text)
        .build();
    let save_default_btn = Button::builder()
        .label("Save as default")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let reset_default_btn = Button::builder()
        .label("Reset to default")
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    default_hint.add_suffix(&save_default_btn);
    default_hint.add_suffix(&reset_default_btn);

    msg_group.add(&message_row);
    msg_group.add(&default_hint);
    content.append(&msg_group);

    {
        let state = state.clone();
        let toasts = toasts.clone();
        let message_row = message_row.clone();
        let default_hint = default_hint.clone();
        let pkg_id = pkg.id.clone();
        save_default_btn.connect_clicked(move |_| {
            let mut text = message_row.text().to_string();
            if text.trim().is_empty() {
                toasts.add_toast(Toast::new("Enter a message before saving."));
                return;
            }
            // De-substitute: if the user typed the package id verbatim, store
            // the template with `{pkg}` so it keeps working for other packages.
            if !text.contains("{pkg}") && text.contains(&pkg_id) {
                text = text.replace(&pkg_id, "{pkg}");
            }
            state.borrow_mut().config.default_commit_message = Some(text.clone());
            if let Err(e) = state.borrow().config.save() {
                toasts.add_toast(Toast::new(&format!("Could not save config: {e}")));
                return;
            }
            default_hint.set_subtitle(&describe_default(&Some(text)));
            toasts.add_toast(Toast::new("Default commit message saved"));
        });
    }

    {
        let state = state.clone();
        let message_row = message_row.clone();
        let toasts = toasts.clone();
        let pkg_id = pkg.id.clone();
        reset_default_btn.connect_clicked(move |_| {
            let tpl = state
                .borrow()
                .config
                .default_commit_message
                .clone()
                .unwrap_or_else(|| FALLBACK_COMMIT_TEMPLATE.to_string());
            message_row.set_text(&config::render_commit_template(&tpl, &pkg_id));
            toasts.add_toast(Toast::new("Reset to default"));
        });
    }

    let stage_btn = Button::builder()
        .label("Prepare (clone + .SRCINFO + diff)")
        .sensitive(ssh_ready)
        .tooltip_text(if ssh_ready {
            "Clones the AUR repo if needed (an empty-repo warning on first clone is \
             normal for a new pkgbase), regenerates .SRCINFO, and shows the diff."
        } else {
            "Set up and verify SSH first."
        })
        .css_classes(vec!["pill"])
        .build();
    let push_btn = Button::builder()
        .label("Commit and push")
        .sensitive(false)
        .tooltip_text(if ssh_ready {
            "Runs after Prepare: commits PKGBUILD + .SRCINFO and pushes to the AUR. \
             A successful push publishes immediately—there is no separate approval step. \
             Enabled only when the clone differs from HEAD."
        } else {
            "Set up and verify SSH first."
        })
        .css_classes(vec!["pill", "destructive-action"])
        .build();

    let spinner = Spinner::new();
    let btn_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .halign(Align::End)
        .build();
    btn_row.append(&spinner);
    btn_row.append(&stage_btn);
    btn_row.append(&push_btn);
    content.append(&btn_row);

    let diff_label = Label::builder()
        .label("Diff vs HEAD")
        .halign(Align::Start)
        .css_classes(vec!["heading"])
        .build();
    content.append(&diff_label);

    let diff_buffer = gtk4::TextBuffer::new(None);
    let diff_view = TextView::builder()
        .buffer(&diff_buffer)
        .editable(false)
        .monospace(true)
        .wrap_mode(WrapMode::None)
        .build();
    let diff_scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .min_content_height(200)
        .vexpand(true)
        .child(&diff_view)
        .build();
    content.append(&diff_scroller);

    let log = LogView::new();
    content.append(log.widget());

    {
        let state = state.clone();
        let spinner = spinner.clone();
        let stage_btn_inner = stage_btn.clone();
        let push_btn = push_btn.clone();
        let log = log.clone();
        let diff_buffer = diff_buffer.clone();
        let toasts = toasts.clone();
        let pkg_st = pkg.clone();
        let ssh_url = pkg.aur_ssh_url();
        stage_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("No working directory configured."));
                return;
            };
            let Some(build_dir) = sync::package_dir(Some(work.as_path()), &pkg_st) else {
                toasts.add_toast(Toast::new(
                    "Could not resolve the package directory — check Sync destination or Connection.",
                ));
                return;
            };
            spinner.start();
            stage_btn_inner.set_sensitive(false);
            log.clear();

            let spinner_done = spinner.clone();
            let stage_btn_done = stage_btn_inner.clone();
            let push_btn = push_btn.clone();
            let log_cb = log.clone();
            let diff_buffer = diff_buffer.clone();
            let toasts = toasts.clone();
            let id = pkg_st.id.clone();
            let ssh_url = ssh_url.clone();
            runtime::spawn_streaming(
                move |tx| async move {
                    let clone_dir = aur_git::ensure_clone(&work, &id, &ssh_url, &tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    build_wf::write_srcinfo(&build_dir, &tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    aur_git::stage_files(&build_dir, &clone_dir)
                        .await
                        .map_err(|e| e.to_string())?;
                    let diff = aur_git::diff(&clone_dir).await.map_err(|e| e.to_string())?;
                    let has_changes = aur_git::has_changes_vs_head(&clone_dir)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok::<_, String>((diff, has_changes))
                },
                move |line| log_cb.append(&line),
                move |res| {
                    spinner_done.stop();
                    stage_btn_done.set_sensitive(true);
                    match res {
                        Ok((diff, has_changes)) => {
                            diff_buffer.set_text(&diff);
                            push_btn.set_sensitive(has_changes);
                            if has_changes {
                                toasts.add_toast(Toast::new(
                                    "Ready to push — review the diff; push publishes immediately",
                                ));
                            } else {
                                toasts.add_toast(Toast::new(
                                    "No changes vs AUR — nothing to commit or push",
                                ));
                            }
                        }
                        Err(e) => {
                            toasts.add_toast(Toast::new(&format!("Failed: {e}")));
                        }
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        let spinner = spinner.clone();
        let push_btn_inner = push_btn.clone();
        let log = log.clone();
        let message_row = message_row.clone();
        let toasts = toasts.clone();
        let id = pkg.id.clone();
        push_btn.connect_clicked(move |_| {
            let Some(work) = state.borrow().config.work_dir.clone() else {
                toasts.add_toast(Toast::new("No working directory configured."));
                return;
            };
            let message = message_row.text().to_string();
            if message.trim().is_empty() {
                toasts.add_toast(Toast::new("Enter a commit message."));
                return;
            }
            spinner.start();
            push_btn_inner.set_sensitive(false);

            let spinner_done = spinner.clone();
            let push_btn_done = push_btn_inner.clone();
            let log_cb = log.clone();
            let toasts = toasts.clone();
            let id = id.clone();
            runtime::spawn_streaming(
                move |tx| async move {
                    let clone_dir = aur_git::aur_clone_dir(&work, &id);
                    aur_git::commit_and_push(&clone_dir, &message, &tx)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |line| log_cb.append(&line),
                move |res| {
                    spinner_done.stop();
                    match res {
                        Ok(()) => {
                            push_btn_done.set_sensitive(false);
                            toasts.add_toast(Toast::new(
                                "Pushed to AUR — run Prepare again after new edits",
                            ));
                        }
                        Err(e) => {
                            push_btn_done.set_sensitive(true);
                            toasts.add_toast(Toast::new(&format!("Push failed: {e}")));
                        }
                    }
                },
            );
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page("Publish", &toasts)
}

fn describe_default(template: &Option<String>) -> String {
    match template {
        Some(t) => format!("{t}   (your saved template)"),
        None => format!("{FALLBACK_COMMIT_TEMPLATE}   (built-in fallback)"),
    }
}
