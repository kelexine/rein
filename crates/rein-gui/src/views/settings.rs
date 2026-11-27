// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive categorized Settings view for rein-gui

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Font, Length};
use rein::config::ProviderKind;

use crate::app::{GuiApp, Message, SettingsCategory, SettingsMessage};
use crate::theme::palette::{
    ACCENT_BLUE, ACCENT_CORAL, ON_PRIMARY_CONTAINER, ON_SECONDARY_CONTAINER, STATUS_GREEN,
    TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{
    active_sidebar_button, composer_input, danger_button, disclosure_card, elevated_card,
    extended_fab, ghost_button, pill_button, primary_button, tonal_pill_button,
};

/// Renders the Material 3 Expressive Settings workspace view.
#[must_use]
pub fn view_settings(app: &GuiApp) -> Element<'_, Message> {
    let header = row![
        column![
            text("Settings").size(22).color(TEXT_PRIMARY),
            text(format!(
                "Global: {} · Workspace: {}",
                app.paths.global_config.display(),
                app.paths.repo_config.display()
            ))
            .size(11)
            .font(Font::MONOSPACE)
            .color(TEXT_DIM),
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        button(text("✕  Back to chat").size(12))
            .padding([7, 16])
            .style(tonal_pill_button)
            .on_press(Message::ToggleSettings),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut cat_rail = column![text("Categories").size(11).color(TEXT_DIM)].spacing(6);
    for cat in SettingsCategory::ALL {
        let active = app.settings.category == cat;
        let label_color = if active {
            ON_SECONDARY_CONTAINER
        } else {
            TEXT_MUTED
        };
        let btn = button(
            row![
                text(cat.glyph()).size(14).color(label_color),
                text(cat.title()).size(13).color(label_color),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([10, 14])
        .style(if active {
            active_sidebar_button
        } else {
            ghost_button
        })
        .on_press(Message::Settings(SettingsMessage::SelectCategory(cat)));
        cat_rail = cat_rail.push(btn);
    }

    let rail_card = container(cat_rail.padding(12))
        .width(Length::Fixed(215.0))
        .style(disclosure_card);

    let detail_pane: Element<'_, Message> = match app.settings.category {
        SettingsCategory::ProviderAndModels => render_provider_and_models(app),
        SettingsCategory::UiAndBehavior => render_ui_and_behavior(app),
        SettingsCategory::SecurityAndAllowlists => render_security_and_allowlists(app),
        SettingsCategory::Integrations => render_integrations(app),
        SettingsCategory::MemoryAndPersonal => render_memory_and_personal(app),
    };

    let mut content_col = column![header].spacing(14);
    if let Some(notice) = &app.settings.status_notice {
        let banner = container(
            row![
                text("●").size(10).color(STATUS_GREEN),
                text(notice).size(12).color(TEXT_PRIMARY),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([8, 14])
        .style(disclosure_card);
        content_col = content_col.push(banner);
    }

    let split = row![
        rail_card,
        scrollable(container(detail_pane).width(Length::Fill).padding([0, 6]))
            .height(Length::Fill)
            .width(Length::Fill),
    ]
    .spacing(18)
    .height(Length::Fill)
    .width(Length::Fill);

    content_col = content_col.push(split);

    container(
        container(content_col)
            .max_width(940.0)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .padding([12, 28])
    .into()
}

fn render_provider_and_models(app: &GuiApp) -> Element<'_, Message> {
    let is_anth = app.provider_kind == ProviderKind::Anthropic;
    let is_agy = app.provider_kind == ProviderKind::Antigravity;

    let provider_pills = row![
        button(
            text(if is_anth {
                "●  Anthropic (Claude API)"
            } else {
                "○  Anthropic (Claude API)"
            })
            .size(13)
        )
        .padding([8, 16])
        .style(if is_anth { primary_button } else { pill_button })
        .on_press(Message::Settings(SettingsMessage::SelectProvider(
            ProviderKind::Anthropic,
        ))),
        button(
            text(if is_agy {
                "●  Google Antigravity (Cloud Code PA)"
            } else {
                "○  Google Antigravity (Cloud Code PA)"
            })
            .size(13)
        )
        .padding([8, 16])
        .style(if is_agy { primary_button } else { pill_button })
        .on_press(Message::Settings(SettingsMessage::SelectProvider(
            ProviderKind::Antigravity,
        ))),
        Space::new().width(Length::Fill),
        button(
            text("Google OAuth Sign-In")
                .size(12)
                .color(ON_PRIMARY_CONTAINER)
        )
        .padding([8, 14])
        .style(extended_fab)
        .on_press(Message::Settings(SettingsMessage::StartOAuthLogin)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut models_col = column![
        row![
            text(format!("Active Model: {}", app.model_label))
                .size(13)
                .color(ACCENT_CORAL),
            Space::new().width(Length::Fill),
            button(text("⟳  Refresh Models").size(12))
                .padding([5, 12])
                .style(tonal_pill_button)
                .on_press(Message::Settings(SettingsMessage::RefreshModels)),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(6);

    if app.available_models.is_empty() {
        models_col = models_col.push(
            text("No cached models for this provider yet. Click Refresh Models or authenticate.")
                .size(12)
                .color(TEXT_DIM),
        );
    } else {
        let mut list = column![].spacing(4);
        for m in app.available_models.iter().take(18) {
            let active = app.model_label.ends_with(&m.id);
            let quota = m
                .quota_percent
                .map_or_else(String::new, |p| format!(" · {p}% quota"));
            let label = format!("{}  ({}{quota})", m.display_name, m.id);
            list = list.push(
                button(text(label).size(12).color(if active {
                    ON_SECONDARY_CONTAINER
                } else {
                    TEXT_PRIMARY
                }))
                .width(Length::Fill)
                .padding([7, 12])
                .style(if active {
                    active_sidebar_button
                } else {
                    ghost_button
                })
                .on_press(Message::SelectModel(m.id.clone())),
            );
        }
        models_col = models_col.push(
            container(scrollable(list).height(Length::Shrink))
                .max_height(190.0)
                .width(Length::Fill),
        );
    }

    let fields_card = container(
        column![
            text("Provider Endpoints & Credentials")
                .size(14)
                .color(TEXT_PRIMARY),
            labeled_input(
                "Max output tokens (empty = provider default)",
                "e.g. 8192",
                &app.settings.max_tokens_input,
                |v| Message::Settings(SettingsMessage::MaxTokensChanged(v)),
            ),
            labeled_input(
                "Anthropic API key environment variable",
                "ANTHROPIC_API_KEY",
                &app.settings.api_key_env_input,
                |v| Message::Settings(SettingsMessage::ApiKeyEnvChanged(v)),
            ),
            labeled_input(
                "Anthropic endpoint override",
                "https://api.anthropic.com/v1/messages",
                &app.settings.endpoint_input,
                |v| Message::Settings(SettingsMessage::EndpointChanged(v)),
            ),
            labeled_input(
                "Antigravity base URL override",
                "https://daily-cloudcode-pa.googleapis.com",
                &app.settings.antigravity_base_url_input,
                |v| Message::Settings(SettingsMessage::AntigravityBaseUrlChanged(v)),
            ),
            labeled_input(
                "Antigravity Cloud companion project ID",
                "aicode-consumers (leave empty for auto-discovery)",
                &app.settings.antigravity_project_id_input,
                |v| Message::Settings(SettingsMessage::AntigravityProjectIdChanged(v)),
            ),
            row![
                Space::new().width(Length::Fill),
                button(text("Save Provider Settings").size(12))
                    .padding([8, 18])
                    .style(primary_button)
                    .on_press(Message::Settings(SettingsMessage::SaveProviderConfig)),
            ],
        ]
        .spacing(10)
        .padding(14),
    )
    .width(Length::Fill)
    .style(elevated_card);

    column![
        container(
            column![
                text("Cloud LLM Provider").size(14).color(TEXT_PRIMARY),
                provider_pills,
            ]
            .spacing(10)
            .padding(14),
        )
        .width(Length::Fill)
        .style(elevated_card),
        container(models_col.padding(14))
            .width(Length::Fill)
            .style(elevated_card),
        fields_card,
    ]
    .spacing(12)
    .into()
}

fn render_ui_and_behavior(app: &GuiApp) -> Element<'_, Message> {
    let s = &app.config.settings;
    let toggles = column![
        text("Transcript & Agent Behavior")
            .size(14)
            .color(TEXT_PRIMARY),
        bool_setting_row(
            "Extended Thinking Mode",
            "Stream model reasoning / thought traces before responses when supported",
            s.thinking_mode,
            "thinking_mode",
        ),
        bool_setting_row(
            "Auto-Compact Long Sessions",
            "Automatically summarize old turns while preserving the verbatim full-transcript archive",
            s.auto_compact,
            "auto_compact",
        ),
        bool_setting_row(
            "Verbose Tool Outputs",
            "Expand tool execution cards and diffs by default in the conversation transcript",
            s.verbose_output,
            "verbose_output",
        ),
        bool_setting_row(
            "Auto-Scroll Transcript",
            "Stick transcript scroll position to the bottom as streaming tokens arrive",
            s.auto_scroll,
            "auto_scroll",
        ),
        bool_setting_row(
            "Show Turn Duration",
            "Display elapsed seconds on completed reasoning and tool turns",
            s.show_turn_duration,
            "show_turn_duration",
        ),
        bool_setting_row(
            "Respect .gitignore in File Picker",
            "Filter out git-ignored paths when searching @ mentions",
            s.respect_gitignore_picker,
            "respect_gitignore_picker",
        ),
        bool_setting_row(
            "Show Contextual Tips",
            "Display quick suggestions and keyboard hints in empty states",
            s.show_tips,
            "show_tips",
        ),
        choice_setting_row(
            "Mid-Turn Message Behavior",
            "Whether prompts submitted while busy steer immediately or queue for next turn",
            &s.queued_messages,
            "queued_messages",
        ),
        choice_setting_row(
            "Composer Editor Keybindings",
            "Input keybinding profile shared with the terminal UI (normal or vi)",
            &s.editor_mode,
            "editor_mode",
        ),
    ]
    .spacing(10)
    .padding(14);

    container(toggles)
        .width(Length::Fill)
        .style(elevated_card)
        .into()
}

fn render_security_and_allowlists(app: &GuiApp) -> Element<'_, Message> {
    let perm_mode = &app.config.settings.default_permission_mode;
    let policy_card = container(
        column![
            row![
                text("Sandbox & Permission Policy")
                    .size(14)
                    .color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                text(format!("Kernel status: {}", app.sandbox_label))
                    .size(12)
                    .color(ACCENT_BLUE),
            ]
            .align_y(Alignment::Center),
            choice_setting_row(
                "Default Permission Mode",
                "policy-gated prompts on outside writes/network/exec; auto-bypass approves all tool calls",
                perm_mode,
                "default_permission_mode",
            ),
        ]
        .spacing(10)
        .padding(14),
    )
    .width(Length::Fill)
    .style(elevated_card);

    let repo_card = render_allowlist_card(
        "Workspace Command Allowlist",
        &app.paths.repo_commands.display().to_string(),
        &app.settings.repo_commands,
        &app.settings.new_repo_command,
        |v| Message::Settings(SettingsMessage::RepoCommandInputChanged(v)),
        Message::Settings(SettingsMessage::AddRepoCommand),
        |pat| Message::Settings(SettingsMessage::RemoveRepoCommand(pat)),
    );

    let global_card = render_allowlist_card(
        "Global Command Allowlist",
        &app.paths.global_commands.display().to_string(),
        &app.settings.global_commands,
        &app.settings.new_global_command,
        |v| Message::Settings(SettingsMessage::GlobalCommandInputChanged(v)),
        Message::Settings(SettingsMessage::AddGlobalCommand),
        |pat| Message::Settings(SettingsMessage::RemoveGlobalCommand(pat)),
    );

    column![policy_card, repo_card, global_card]
        .spacing(12)
        .into()
}

fn render_allowlist_card<'a>(
    title: &'a str,
    subtitle_path: &str,
    patterns: &'a [String],
    input_val: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
    on_add: Message,
    on_remove: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let mut list = column![].spacing(4);
    if patterns.is_empty() {
        list = list.push(
            text("No allowlisted command patterns.")
                .size(12)
                .color(TEXT_DIM),
        );
    } else {
        for pat in patterns {
            list = list.push(
                container(
                    row![
                        text(pat).size(12).font(Font::MONOSPACE).color(TEXT_PRIMARY),
                        Space::new().width(Length::Fill),
                        button(text("Remove").size(11))
                            .padding([3, 10])
                            .style(danger_button)
                            .on_press(on_remove(pat.clone())),
                    ]
                    .align_y(Alignment::Center),
                )
                .padding([6, 10])
                .style(disclosure_card),
            );
        }
    }

    let add_row = row![
        container(
            text_input(
                "Add pattern (e.g. cargo *, git status, tool:write)…",
                input_val
            )
            .on_input(on_input)
            .on_submit(on_add.clone())
            .padding([6, 10])
            .size(12)
            .style(composer_input),
        )
        .width(Length::Fill)
        .style(disclosure_card),
        button(text("+  Add").size(12))
            .padding([7, 14])
            .style(primary_button)
            .on_press(on_add),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        column![
            text(title).size(14).color(TEXT_PRIMARY),
            text(subtitle_path.to_owned())
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_DIM),
            list,
            add_row,
        ]
        .spacing(8)
        .padding(14),
    )
    .width(Length::Fill)
    .style(elevated_card)
    .into()
}

fn render_integrations(app: &GuiApp) -> Element<'_, Message> {
    let mut mcp_list = column![].spacing(6);
    if app.config.mcp_servers.is_empty() {
        mcp_list = mcp_list.push(
            text("No stdio MCP servers configured.")
                .size(12)
                .color(TEXT_DIM),
        );
    } else {
        for srv in &app.config.mcp_servers {
            let cmd_line = if srv.args.is_empty() {
                srv.command.clone()
            } else {
                format!("{} {}", srv.command, srv.args.join(" "))
            };
            mcp_list = mcp_list.push(
                container(
                    row![
                        column![
                            text(&srv.name).size(13).color(TEXT_PRIMARY),
                            text(cmd_line)
                                .size(11)
                                .font(Font::MONOSPACE)
                                .color(TEXT_DIM),
                        ]
                        .spacing(2),
                        Space::new().width(Length::Fill),
                        button(text("Remove").size(11))
                            .padding([4, 10])
                            .style(danger_button)
                            .on_press(Message::Settings(SettingsMessage::RemoveMcpServer(
                                srv.name.clone(),
                            ))),
                    ]
                    .align_y(Alignment::Center),
                )
                .padding([8, 12])
                .style(disclosure_card),
            );
        }
    }

    let mcp_add = column![
        row![
            container(
                text_input(
                    "Server name (e.g. filesystem)",
                    &app.settings.mcp_name_input
                )
                .on_input(|v| Message::Settings(SettingsMessage::McpNameChanged(v)))
                .padding([6, 10])
                .size(12)
                .style(composer_input),
            )
            .width(Length::Fixed(180.0))
            .style(disclosure_card),
            container(
                text_input("Command (e.g. npx)", &app.settings.mcp_command_input)
                    .on_input(|v| Message::Settings(SettingsMessage::McpCommandChanged(v)))
                    .padding([6, 10])
                    .size(12)
                    .style(composer_input),
            )
            .width(Length::Fixed(150.0))
            .style(disclosure_card),
            container(
                text_input("Arguments (space-separated)", &app.settings.mcp_args_input)
                    .on_input(|v| Message::Settings(SettingsMessage::McpArgsChanged(v)))
                    .padding([6, 10])
                    .size(12)
                    .style(composer_input),
            )
            .width(Length::Fill)
            .style(disclosure_card),
            button(text("+  Add MCP").size(12))
                .padding([7, 14])
                .style(primary_button)
                .on_press(Message::Settings(SettingsMessage::AddMcpServer)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ];

    let mcp_card = container(
        column![
            text("Model Context Protocol (MCP) Servers")
                .size(14)
                .color(TEXT_PRIMARY),
            mcp_list,
            mcp_add,
        ]
        .spacing(10)
        .padding(14),
    )
    .width(Length::Fill)
    .style(elevated_card);

    let mut plugins_col = column![
        row![
            text("Claude-Compatible Plugins & Marketplaces")
                .size(14)
                .color(TEXT_PRIMARY),
            Space::new().width(Length::Fill),
            button(text("⟳  Sync Marketplaces").size(12))
                .padding([5, 12])
                .style(tonal_pill_button)
                .on_press(Message::Settings(SettingsMessage::RefreshMarketplaces)),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(8);

    for mp in &app.settings.marketplaces {
        plugins_col = plugins_col.push(
            container(
                row![
                    column![
                        text(&mp.name).size(12).color(TEXT_PRIMARY),
                        text(&mp.url).size(11).font(Font::MONOSPACE).color(TEXT_DIM),
                    ],
                    Space::new().width(Length::Fill),
                    button(text("Remove").size(11))
                        .padding([3, 10])
                        .style(ghost_button)
                        .on_press(Message::Settings(SettingsMessage::RemoveMarketplace(
                            mp.name.clone(),
                        ))),
                ]
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .style(disclosure_card),
        );
    }

    let mp_add_row = row![
        container(
            text_input(
                "Add marketplace (owner/repo or git URL)…",
                &app.settings.marketplace_input
            )
            .on_input(|v| Message::Settings(SettingsMessage::MarketplaceInputChanged(v)))
            .padding([6, 10])
            .size(12)
            .style(composer_input),
        )
        .width(Length::Fill)
        .style(disclosure_card),
        button(text("+  Add Marketplace").size(12))
            .padding([7, 14])
            .style(pill_button)
            .on_press(Message::Settings(SettingsMessage::AddMarketplace)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    plugins_col = plugins_col.push(mp_add_row);

    if !app.settings.installed_plugins.is_empty() {
        plugins_col = plugins_col.push(text("Installed Plugins").size(12).color(TEXT_MUTED));
        for rec in &app.settings.installed_plugins {
            let toggle_label = if rec.enabled { "Enabled" } else { "Disabled" };
            plugins_col = plugins_col.push(
                container(
                    row![
                        column![
                            text(&rec.name).size(13).color(TEXT_PRIMARY),
                            text(&rec.description).size(11).color(TEXT_DIM),
                        ],
                        Space::new().width(Length::Fill),
                        button(text(toggle_label).size(11))
                            .padding([4, 12])
                            .style(if rec.enabled {
                                tonal_pill_button
                            } else {
                                pill_button
                            })
                            .on_press(Message::Settings(SettingsMessage::TogglePlugin(
                                rec.name.clone(),
                            ))),
                        button(text("Uninstall").size(11))
                            .padding([4, 10])
                            .style(danger_button)
                            .on_press(Message::Settings(SettingsMessage::UninstallPlugin(
                                rec.name.clone(),
                            ))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding([8, 10])
                .style(disclosure_card),
            );
        }
    }

    if !app.settings.discover_plugins.is_empty() {
        plugins_col = plugins_col.push(text("Marketplace Catalog").size(12).color(TEXT_MUTED));
        for entry in app.settings.discover_plugins.iter().take(12) {
            if entry.installed {
                continue;
            }
            plugins_col = plugins_col.push(
                container(
                    row![
                        column![
                            text(format!("{} ({})", entry.name, entry.marketplace))
                                .size(12)
                                .color(TEXT_PRIMARY),
                            text(&entry.description).size(11).color(TEXT_DIM),
                        ],
                        Space::new().width(Length::Fill),
                        button(text("Install").size(11))
                            .padding([4, 12])
                            .style(primary_button)
                            .on_press(Message::Settings(SettingsMessage::InstallPlugin(
                                entry.name.clone(),
                            ))),
                    ]
                    .align_y(Alignment::Center),
                )
                .padding([6, 10])
                .style(disclosure_card),
            );
        }
    }

    let plugins_card = container(plugins_col.padding(14))
        .width(Length::Fill)
        .style(elevated_card);

    column![mcp_card, plugins_card].spacing(12).into()
}

fn render_memory_and_personal(app: &GuiApp) -> Element<'_, Message> {
    let dream_toggle = row![
        column![
            text("Enable AutoDream Memory Consolidation")
                .size(13)
                .color(TEXT_PRIMARY),
            text("Background Ebbinghaus salience decay and vault index consolidation")
                .size(11)
                .color(TEXT_DIM),
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        button(
            text(if app.settings.dream_enabled {
                "ON"
            } else {
                "OFF"
            })
            .size(12),
        )
        .padding([6, 16])
        .style(if app.settings.dream_enabled {
            primary_button
        } else {
            pill_button
        })
        .on_press(Message::Settings(SettingsMessage::ToggleDreamEnabled)),
    ]
    .align_y(Alignment::Center);

    let card = container(
        column![
            text("AutoDream & Personal Mode Resources")
                .size(14)
                .color(TEXT_PRIMARY),
            dream_toggle,
            labeled_input(
                "Minimum hours between AutoDream consolidation passes",
                "24",
                &app.settings.dream_min_hours_input,
                |v| Message::Settings(SettingsMessage::DreamMinHoursChanged(v)),
            ),
            labeled_input(
                "Minimum modified sessions required to trigger AutoDream",
                "5",
                &app.settings.dream_min_sessions_input,
                |v| Message::Settings(SettingsMessage::DreamMinSessionsChanged(v)),
            ),
            labeled_input(
                "Maximum tool turns per AutoDream pass",
                "12",
                &app.settings.dream_max_turns_input,
                |v| Message::Settings(SettingsMessage::DreamMaxTurnsChanged(v)),
            ),
            labeled_input(
                "Personal mode external .ics calendar directory",
                "~/.rein/calendar",
                &app.settings.ics_dir_input,
                |v| Message::Settings(SettingsMessage::IcsDirChanged(v)),
            ),
            labeled_input(
                "Personal mode local Maildir root (new/, cur/)",
                "~/.rein/maildir",
                &app.settings.maildir_input,
                |v| Message::Settings(SettingsMessage::MaildirChanged(v)),
            ),
            row![
                Space::new().width(Length::Fill),
                button(text("Save Memory & Personal Settings").size(12))
                    .padding([8, 18])
                    .style(primary_button)
                    .on_press(Message::Settings(
                        SettingsMessage::SaveMemoryAndPersonalConfig
                    )),
            ],
        ]
        .spacing(10)
        .padding(14),
    )
    .width(Length::Fill)
    .style(elevated_card);

    column![card].spacing(12).into()
}

fn bool_setting_row<'a>(
    title: &'a str,
    subtitle: &'a str,
    enabled: bool,
    key: &'static str,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(title).size(13).color(TEXT_PRIMARY),
                text(subtitle).size(11).color(TEXT_DIM),
            ]
            .spacing(2),
            Space::new().width(Length::Fill),
            button(text(if enabled { "ON" } else { "OFF" }).size(12))
                .padding([6, 16])
                .style(if enabled { primary_button } else { pill_button })
                .on_press(Message::Settings(SettingsMessage::ToggleUiBool(key))),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .style(disclosure_card)
    .into()
}

fn choice_setting_row<'a>(
    title: &'a str,
    subtitle: &'a str,
    current_value: &'a str,
    key: &'static str,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(title).size(13).color(TEXT_PRIMARY),
                text(subtitle).size(11).color(TEXT_DIM),
            ]
            .spacing(2),
            Space::new().width(Length::Fill),
            button(text(current_value).size(12))
                .padding([6, 16])
                .style(tonal_pill_button)
                .on_press(Message::Settings(SettingsMessage::CycleUiChoice(key))),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .style(disclosure_card)
    .into()
}

fn labeled_input<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    column![
        text(label).size(11).color(TEXT_MUTED),
        container(
            text_input(placeholder, value)
                .on_input(on_input)
                .padding([7, 10])
                .size(12)
                .style(composer_input),
        )
        .width(Length::Fill)
        .style(disclosure_card),
    ]
    .spacing(4)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_categories_have_distinct_titles_and_glyphs() {
        assert_eq!(SettingsCategory::ALL.len(), 5);
        for cat in SettingsCategory::ALL {
            assert!(!cat.title().is_empty());
            assert!(!cat.glyph().is_empty());
        }
    }
}
