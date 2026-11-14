//! Connection profile sidebar: profile management, broker settings,
//! authentication, TLS, subscriptions and Last Will & Testament.

use crate::client::ClientCommand;
use crate::config::{uuid_short, AppConfig, ConnectionProfile, SubscriptionRule};
use crate::protocol::{MqttVersion, QoS};
use crate::{ConnectionState, MqttApp};
use eframe::egui::{self, RichText, ScrollArea, Ui};

impl MqttApp {
    pub(crate) fn render_profile_sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Connection Profiles").size(15.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Save")
                    .on_hover_text("Save profiles to mqtt_profiles.toml")
                    .clicked()
                {
                    match self.config.save_to_file(&AppConfig::default_config_path()) {
                        Ok(_) => self.log("Saved profiles to mqtt_profiles.toml"),
                        Err(e) => self.log(e),
                    }
                }
                if ui
                    .small_button("Export")
                    .on_hover_text("Export profiles to TOML or JSON file")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML / JSON Config", &["toml", "json"])
                        .set_file_name("mqtt_profiles.toml")
                        .save_file()
                    {
                        match self.config.save_to_file(&path) {
                            Ok(_) => self.log(format!("Exported config to {}", path.display())),
                            Err(e) => self.log(e),
                        }
                    }
                }
                if ui
                    .small_button("Import")
                    .on_hover_text("Import profiles from TOML or JSON file")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML / JSON Config", &["toml", "json"])
                        .pick_file()
                    {
                        match AppConfig::load_from_file(&path) {
                            Ok(loaded) => {
                                if !loaded.profiles.is_empty() {
                                    self.config = loaded;
                                    self.log(format!("Imported profiles from {}", path.display()));
                                }
                            }
                            Err(e) => self.log(e),
                        }
                    }
                }
            });
        });

        ui.horizontal_wrapped(|ui| {
            if ui.button("+ New Profile").clicked() {
                let mut p = ConnectionProfile::default();
                p.name = format!("Broker {}", self.config.profiles.len() + 1);
                self.config.profiles.push(p);
                self.config.selected_profile_idx = self.config.profiles.len() - 1;
            }
            if ui.button("Duplicate").clicked() {
                let mut dup = self.current_profile().clone();
                dup.id = uuid_short();
                dup.name = format!("{} (Copy)", dup.name);
                dup.client_id = format!("mqtt-rs-{}", uuid_short());
                self.config.profiles.push(dup);
                self.config.selected_profile_idx = self.config.profiles.len() - 1;
            }
            if self.config.profiles.len() > 1 && ui.button("Delete").clicked() {
                self.config
                    .profiles
                    .remove(self.config.selected_profile_idx);
                if self.config.selected_profile_idx >= self.config.profiles.len() {
                    self.config.selected_profile_idx = self.config.profiles.len() - 1;
                }
            }
        });

        ui.separator();

        ScrollArea::vertical()
            .id_salt("profile_settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut show_pw = self.show_password;
                let profile = self.current_profile_mut();

                egui::CollapsingHeader::new(RichText::new("Connection Profile").strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("broker_basic_grid")
                            .num_columns(2)
                            .spacing([8.0, 6.0])
                            .striped(false)
                            .show(ui, |ui| {
                                ui.label("Profile Name:");
                                ui.text_edit_singleline(&mut profile.name);
                                ui.end_row();

                                ui.label("Protocol:");
                                egui::ComboBox::from_id_salt("proto_ver_combo")
                                    .selected_text(profile.version.label())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut profile.version,
                                            MqttVersion::V31,
                                            MqttVersion::V31.label(),
                                        );
                                        ui.selectable_value(
                                            &mut profile.version,
                                            MqttVersion::V311,
                                            MqttVersion::V311.label(),
                                        );
                                        ui.selectable_value(
                                            &mut profile.version,
                                            MqttVersion::V5,
                                            MqttVersion::V5.label(),
                                        );
                                    });
                                ui.end_row();

                                ui.label("Host:");
                                ui.text_edit_singleline(&mut profile.host);
                                ui.end_row();

                                ui.label("Port:");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut profile.port).range(1..=65535),
                                    );
                                    if ui.small_button("1883").clicked() {
                                        profile.port = 1883;
                                        profile.tls.enabled = false;
                                    }
                                    if ui.small_button("8883 (TLS)").clicked() {
                                        profile.port = 8883;
                                        profile.tls.enabled = true;
                                    }
                                });
                                ui.end_row();

                                ui.label("Client ID:");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut profile.client_id)
                                            .desired_width(145.0),
                                    );
                                    if ui
                                        .small_button("Rand")
                                        .on_hover_text("Generate random Client ID")
                                        .clicked()
                                    {
                                        profile.client_id = format!("mqtt-rs-{}", uuid_short());
                                    }
                                });
                                ui.end_row();

                                ui.label("Session:");
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut profile.clean_session, "Clean Start");
                                    ui.label("KeepAlive:");
                                    ui.add(
                                        egui::DragValue::new(&mut profile.keep_alive_secs)
                                            .range(5..=3600)
                                            .suffix("s"),
                                    );
                                });
                                ui.end_row();
                            });
                    });

                ui.add_space(6.0);
                egui::CollapsingHeader::new(
                    RichText::new("Authentication (Username / Password)").strong(),
                )
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("auth_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Username:");
                            ui.text_edit_singleline(&mut profile.username);
                            ui.end_row();

                            ui.label("Password:");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut profile.password)
                                        .password(!show_pw)
                                        .desired_width(145.0),
                                );
                                if ui
                                    .small_button(if show_pw { "Hide" } else { "Show" })
                                    .clicked()
                                {
                                    show_pw = !show_pw;
                                }
                            });
                            ui.end_row();
                        });
                });
                self.show_password = show_pw;

                let profile = self.current_profile_mut();
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("TLS / Certificates (rustls)").strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.checkbox(&mut profile.tls.enabled, "Enable TLS / SSL (MQTT over TLS)");
                        if profile.tls.enabled {
                            ui.checkbox(
                                &mut profile.tls.accept_invalid_certs,
                                "Skip certificate verification (insecure / self-signed)",
                            );

                            ui.add_space(4.0);
                            ui.label("Custom CA Certificate (.pem / .crt / .der):");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut profile.tls.ca_cert_path)
                                        .hint_text("Optional custom CA path...")
                                        .desired_width(190.0),
                                );
                                if ui.button("Browse...").clicked() {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("Certificates", &["pem", "crt", "cer", "der"])
                                        .pick_file()
                                    {
                                        profile.tls.ca_cert_path = path.display().to_string();
                                    }
                                }
                            });

                            egui::CollapsingHeader::new("Optional Mutual TLS (Client Cert & Key)")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.label("Client Certificate (.pem):");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut profile.tls.client_cert_path,
                                            )
                                            .desired_width(180.0),
                                        );
                                        if ui.small_button("Browse...").clicked() {
                                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                                profile.tls.client_cert_path =
                                                    p.display().to_string();
                                            }
                                        }
                                    });
                                    ui.label("Client Private Key (.pem):");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut profile.tls.client_key_path,
                                            )
                                            .desired_width(180.0),
                                        );
                                        if ui.small_button("Browse...").clicked() {
                                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                                profile.tls.client_key_path =
                                                    p.display().to_string();
                                            }
                                        }
                                    });
                                });
                        }
                    });

                ui.add_space(6.0);
                self.render_subscriptions_section(ui);

                let profile = self.current_profile_mut();
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("Last Will & Testament (LWT)").strong())
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.checkbox(&mut profile.last_will_enabled, "Enable Last Will");
                        if profile.last_will_enabled {
                            ui.label("Will Topic:");
                            ui.text_edit_singleline(&mut profile.last_will.topic);
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("lwt_qos")
                                    .selected_text(profile.last_will.qos.short_label())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::AtMostOnce,
                                            "QoS 0",
                                        );
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::AtLeastOnce,
                                            "QoS 1",
                                        );
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::ExactlyOnce,
                                            "QoS 2",
                                        );
                                    });
                                ui.checkbox(&mut profile.last_will.retain, "Retain");
                            });
                        }
                    });
            });
    }

    fn render_subscriptions_section(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new(RichText::new("Subscriptions").strong())
            .default_open(true)
            .show(ui, |ui| {
                let mut remove_idx = None;
                let mut toggle_sub: Option<(String, QoS, bool)> = None;
                let is_connected = self.conn_state == ConnectionState::Connected;

                let subs = &mut self.current_profile_mut().subscriptions;
                for (i, sub) in subs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut sub.enabled, "").changed() && is_connected {
                            toggle_sub = Some((sub.topic_filter.clone(), sub.qos, sub.enabled));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut sub.topic_filter).desired_width(130.0),
                        );
                        egui::ComboBox::from_id_salt(format!("sub_qos_{}", i))
                            .width(62.0)
                            .selected_text(sub.qos.short_label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut sub.qos, QoS::AtMostOnce, "QoS 0");
                                ui.selectable_value(&mut sub.qos, QoS::AtLeastOnce, "QoS 1");
                                ui.selectable_value(&mut sub.qos, QoS::ExactlyOnce, "QoS 2");
                            });
                        if ui.small_button("x").clicked() {
                            remove_idx = Some(i);
                        }
                    });
                }

                if let Some(i) = remove_idx {
                    let removed = self.current_profile_mut().subscriptions.remove(i);
                    if let Some(tx) = &self.cmd_tx {
                        let _ = tx.send(ClientCommand::Unsubscribe {
                            topic_filter: removed.topic_filter,
                        });
                    }
                }

                if let Some((filter, qos, en)) = toggle_sub {
                    if let Some(tx) = &self.cmd_tx {
                        if en {
                            let _ = tx.send(ClientCommand::Subscribe {
                                topic_filter: filter,
                                qos,
                            });
                        } else {
                            let _ = tx.send(ClientCommand::Unsubscribe {
                                topic_filter: filter,
                            });
                        }
                    }
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_sub_filter)
                            .hint_text("sensors/#")
                            .desired_width(130.0),
                    );
                    egui::ComboBox::from_id_salt("new_sub_qos")
                        .width(62.0)
                        .selected_text(self.new_sub_qos.short_label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.new_sub_qos, QoS::AtMostOnce, "QoS 0");
                            ui.selectable_value(&mut self.new_sub_qos, QoS::AtLeastOnce, "QoS 1");
                            ui.selectable_value(&mut self.new_sub_qos, QoS::ExactlyOnce, "QoS 2");
                        });
                    if ui.button("+ Add").clicked() && !self.new_sub_filter.trim().is_empty() {
                        let filter = self.new_sub_filter.trim().to_string();
                        let qos = self.new_sub_qos;
                        self.current_profile_mut()
                            .subscriptions
                            .push(SubscriptionRule {
                                topic_filter: filter.clone(),
                                qos,
                                enabled: true,
                            });
                        if let Some(tx) = &self.cmd_tx {
                            let _ = tx.send(ClientCommand::Subscribe {
                                topic_filter: filter,
                                qos,
                            });
                        }
                        self.new_sub_filter.clear();
                    }
                });
            });
    }
}
