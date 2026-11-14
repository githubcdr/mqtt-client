//! Publish drawer: topic, QoS, retain, encoding, MQTT v5 properties and the
//! payload editor.

use crate::payload::PublishEncoding;
use crate::protocol::QoS;
use crate::MqttApp;
use eframe::egui::{self, Color32, RichText, Ui};

impl MqttApp {
    pub(crate) fn render_publish_drawer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Topic:");
            ui.add(
                egui::TextEdit::singleline(&mut self.publish_draft.topic)
                    .hint_text("sensors/temperature")
                    .desired_width(220.0),
            );

            egui::ComboBox::from_id_salt("pub_qos_combo")
                .width(70.0)
                .selected_text(self.publish_draft.qos.short_label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::AtMostOnce, "QoS 0");
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::AtLeastOnce, "QoS 1");
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::ExactlyOnce, "QoS 2");
                });

            ui.checkbox(&mut self.publish_draft.retain, "Retain");

            let pub_btn =
                egui::Button::new(RichText::new("Publish").color(Color32::WHITE).strong())
                    .fill(Color32::from_rgb(45, 115, 210));
            if ui.add(pub_btn).clicked() {
                self.execute_publish();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Encode as:");
            egui::ComboBox::from_id_salt("pub_enc_combo")
                .selected_text(self.publish_draft.encoding.label())
                .show_ui(ui, |ui| {
                    for enc in [
                        PublishEncoding::Text,
                        PublishEncoding::Json,
                        PublishEncoding::Hex,
                        PublishEncoding::Base64,
                        PublishEncoding::MessagePackFromJson,
                        PublishEncoding::ProtobufFromJson,
                    ] {
                        ui.selectable_value(&mut self.publish_draft.encoding, enc, enc.label());
                    }
                });

            ui.checkbox(&mut self.publish_draft.show_v5_props, "MQTT v5 Properties");
        });

        if self.publish_draft.show_v5_props {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(25, 30, 40))
                .show(ui, |ui| {
                    egui::Grid::new("pub_v5_props_grid")
                        .num_columns(4)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Content-Type:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.content_type)
                                    .desired_width(130.0),
                            );
                            ui.label("Expiry (s):");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.message_expiry)
                                    .hint_text("e.g. 3600")
                                    .desired_width(80.0),
                            );
                            ui.end_row();

                            ui.label("Response-Topic:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.response_topic)
                                    .desired_width(130.0),
                            );
                            ui.label("Correlation:");
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.publish_draft.correlation_data,
                                )
                                .desired_width(80.0),
                            );
                            ui.end_row();
                        });

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("User Properties:").size(11.5).strong());
                        if ui.small_button("+ Add Property").clicked() {
                            self.publish_draft
                                .user_properties
                                .push((String::new(), String::new()));
                        }
                    });

                    let mut remove_prop = None;
                    for (idx, (k, v)) in self.publish_draft.user_properties.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(k)
                                    .hint_text("key")
                                    .desired_width(110.0),
                            );
                            ui.label("=");
                            ui.add(
                                egui::TextEdit::singleline(v)
                                    .hint_text("value")
                                    .desired_width(150.0),
                            );
                            if ui.small_button("x").clicked() {
                                remove_prop = Some(idx);
                            }
                        });
                    }
                    if let Some(idx) = remove_prop {
                        self.publish_draft.user_properties.remove(idx);
                    }
                });
        }

        ui.add(
            egui::TextEdit::multiline(&mut self.publish_draft.payload_input)
                .font(egui::TextStyle::Monospace)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
    }
}
