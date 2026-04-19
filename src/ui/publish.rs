use adw::prelude::*;
use adw::{ActionRow, Banner, EntryRow, NavigationPage, Toast, ToastOverlay};
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PolicyType, ScrolledWindow, Spinner,
    TextView, WrapMode,
};

use crate::config::{self, FALLBACK_COMMIT_TEMPLATE};
use crate::i18n;
use crate::runtime;
use crate::state::AppStateRef;
use crate::ui;
use crate::ui::log_view::LogView;
use crate::ui::shell::MainShell;
use crate::workflow::admin;
use crate::workflow::aur_git;
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
        .label(i18n::tf(
            "publish.heading",
            &[("title", pkg.title.as_str())],
        ))
        .halign(Align::Start)
        .css_classes(vec!["title-2"])
        .build();
    content.append(&heading);

    let ssh_url = pkg.aur_ssh_url();
    let sub = Label::builder()
        .label(i18n::tf("publish.subtitle", &[("url", ssh_url.as_str())]))
        .halign(Align::Start)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["dim-label"])
        .build();
    content.append(&sub);

    let expectations_sub = i18n::t("publish.expectations_sub");
    content.append(&ui::collapsible_preferences_section(
        i18n::t("publish.expectations_title"),
        Some(expectations_sub.as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |_exp| {},
    ));

    // SSH must be verified before we can git-clone/push over ssh://aur@…
    let ssh_ready = state.borrow().ssh_ok;
    if !ssh_ready {
        let banner = Banner::builder()
            .title(i18n::t("publish.banner_ssh_title"))
            .button_label(i18n::t("register.banner_ssh_btn"))
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

    let message_row = EntryRow::builder()
        .title(i18n::t("publish.commit_message_row_title"))
        .build();

    let initial_template = state
        .borrow()
        .config
        .default_commit_message
        .clone()
        .unwrap_or_else(|| FALLBACK_COMMIT_TEMPLATE.to_string());
    message_row.set_text(&config::render_commit_template(&initial_template, &pkg.id));

    let default_text = describe_default(&state.borrow().config.default_commit_message);
    let default_hint = ActionRow::builder()
        .title(i18n::t("publish.default_template_row_title"))
        .subtitle(default_text)
        .build();
    let save_default_btn = Button::builder()
        .label(i18n::t("publish.save_default_btn"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    let reset_default_btn = Button::builder()
        .label(i18n::t("publish.reset_default_btn"))
        .valign(Align::Center)
        .css_classes(vec!["pill"])
        .build();
    default_hint.add_suffix(&save_default_btn);
    default_hint.add_suffix(&reset_default_btn);

    let commit_section_sub = i18n::t("publish.commit_section_sub");
    content.append(&ui::collapsible_preferences_section(
        i18n::t("publish.commit_section_title"),
        Some(commit_section_sub.as_str()),
        ui::DEFAULT_SECTION_EXPANDED,
        |exp| {
            exp.add_row(&message_row);
            exp.add_row(&default_hint);
        },
    ));

    {
        let state = state.clone();
        let toasts = toasts.clone();
        let message_row = message_row.clone();
        let default_hint = default_hint.clone();
        let pkg_id = pkg.id.clone();
        save_default_btn.connect_clicked(move |_| {
            let mut text = message_row.text().to_string();
            if text.trim().is_empty() {
                toasts.add_toast(Toast::new(&i18n::t("publish.toast_enter_message")));
                return;
            }
            // De-substitute: if the user typed the package id verbatim, store
            // the template with `{pkg}` so it keeps working for other packages.
            if !text.contains("{pkg}") && text.contains(&pkg_id) {
                text = text.replace(&pkg_id, "{pkg}");
            }
            state.borrow_mut().config.default_commit_message = Some(text.clone());
            if let Err(e) = state.borrow().config.save() {
                toasts.add_toast(Toast::new(&i18n::tf(
                    "publish.toast_save_config_failed",
                    &[("e", &e.to_string())],
                )));
                return;
            }
            default_hint.set_subtitle(&describe_default(&Some(text)));
            toasts.add_toast(Toast::new(&i18n::t("publish.toast_default_saved")));
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
            toasts.add_toast(Toast::new(&i18n::t("publish.toast_reset_default")));
        });
    }

    let stage_btn = Button::builder()
        .label(i18n::t("publish.stage_btn"))
        .sensitive(ssh_ready)
        .tooltip_text(if ssh_ready {
            i18n::t("publish.stage_tooltip_ok")
        } else {
            i18n::t("publish.stage_tooltip_no_ssh")
        })
        .css_classes(vec!["pill"])
        .build();
    let push_btn = Button::builder()
        .label(i18n::t("publish.push_btn"))
        .sensitive(false)
        .tooltip_text(if ssh_ready {
            i18n::t("publish.push_tooltip_ok")
        } else {
            i18n::t("publish.push_tooltip_no_ssh")
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
        .label(i18n::t("publish.diff_heading"))
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

    let log = LogView::new(
        i18n::t("publish.log_title"),
        i18n::t("publish.log_subtitle"),
    );
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
                toasts.add_toast(Toast::new(&i18n::t("publish.toast_no_workdir")));
                return;
            };
            let Some(build_dir) = sync::package_dir(Some(work.as_path()), &pkg_st) else {
                toasts.add_toast(Toast::new(&i18n::t("publish.toast_could_not_resolve_dir")));
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
                    admin::prepare_pkgdir_for_aur_push(&build_dir, Some(ssh_url.as_str()), &tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    let clone_dir = aur_git::ensure_clone(&work, &id, &ssh_url, &tx)
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
                                toasts.add_toast(Toast::new(&i18n::t("publish.toast_push_ready")));
                            } else {
                                toasts.add_toast(Toast::new(&i18n::t(
                                    "publish.toast_no_changes_vs_aur",
                                )));
                            }
                        }
                        Err(e) => {
                            toasts.add_toast(Toast::new(&i18n::tf("manage.failed", &[("e", &e)])));
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
                toasts.add_toast(Toast::new(&i18n::t("publish.toast_no_workdir")));
                return;
            };
            let message = message_row.text().to_string();
            if message.trim().is_empty() {
                toasts.add_toast(Toast::new(&i18n::t("publish.toast_no_message")));
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
                            toasts.add_toast(Toast::new(&i18n::t("publish.toast_pushed")));
                        }
                        Err(e) => {
                            push_btn_done.set_sensitive(true);
                            toasts.add_toast(Toast::new(&i18n::tf(
                                "publish.toast_push_failed",
                                &[("e", &e)],
                            )));
                        }
                    }
                },
            );
        });
    }

    toasts.set_child(Some(&content));
    ui::home::wrap_page(&i18n::t("publish.page_title"), &toasts)
}

fn describe_default(template: &Option<String>) -> String {
    match template {
        Some(t) => i18n::tf("publish.default_desc_saved", &[("template", t.as_str())]),
        None => i18n::tf(
            "publish.default_desc_builtin",
            &[("template", FALLBACK_COMMIT_TEMPLATE)],
        ),
    }
}
