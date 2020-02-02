use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

//mod message_information;
use crate::message_information::MessageInformation;

pub struct Vehicle {
    channel: std::sync::Arc<std::boxed::Box<(dyn mavlink::MavConnection + std::marker::Send + std::marker::Sync + 'static)>>,
    messages: std::sync::Arc<Mutex<serde_json::value::Value>>,
    verbose: bool,
    // Create threads
    heartbeat_thread: thread::JoinHandle<()>,
    parser_thread: thread::JoinHandle<()>,
}

struct InnerVehicle { inner: Arc<Mutex<Vehicle>> }

impl Vehicle {
    pub fn connect(&mut self, connection_string: &str) {
        let mavconn = mavlink::connect(connection_string).unwrap();
        self.channel = Arc::new(mavconn);
    }

    pub fn run(&mut self) {
        let _ = self.channel.send_default(&Vehicle::request_stream());

        self.heartbeat_thread = thread::spawn(|| {self.heartbeat_loop()});
        self.parser_thread = thread::spawn(|| {self.parser_loop()});
    }

    fn heartbeat_loop(&self) {
        let vehicle = self.channel.clone();
        move || loop {
            let res = vehicle.send_default(&Vehicle::heartbeat_message());
            if res.is_err() {
                println!("Failed to send heartbeat");
            }
            thread::sleep(Duration::from_secs(1));
        };
    }

    fn parser_loop(&mut self) {
        let vehicle = self.channel.clone();
        let messages_ref = Arc::clone(&self.messages);

        let mut messages_information: std::collections::HashMap<
            std::string::String,
            MessageInformation,
        > = std::collections::HashMap::new();
        move || {
            loop {
                match vehicle.recv() {
                    Ok((_header, msg)) => {
                        let value = serde_json::to_value(&msg).unwrap();
                        let mut msgs = messages_ref.lock().unwrap();
                        // Remove " from string
                        let msg_type = value["type"].to_string().replace("\"", "");
                        msgs["mavlink"][&msg_type] = value;
                        if self.verbose {
                            println!("Got: {}", msg_type);
                        }

                        // Update message_information
                        let message_information = messages_information
                            .entry(msg_type.clone())
                            .or_insert(MessageInformation::default());
                        message_information.update();
                        msgs["mavlink"][&msg_type]["message_information"] =
                            serde_json::to_value(messages_information[&msg_type]).unwrap();
                    }
                    Err(e) => {
                        match e.kind() {
                            std::io::ErrorKind::WouldBlock => {
                                //no messages currently available to receive -- wait a while
                                thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                            _ => {
                                println!("recv error: {:?}", e);
                                break;
                            }
                        }
                    }
                }
            }
        };
    }

    fn heartbeat_message() -> mavlink::common::MavMessage {
        mavlink::common::MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: mavlink::common::MavModeFlag::empty(),
            system_status: mavlink::common::MavState::MAV_STATE_STANDBY,
            mavlink_version: 0x3,
        })
    }

    fn request_stream() -> mavlink::common::MavMessage {
        mavlink::common::MavMessage::REQUEST_DATA_STREAM(mavlink::common::REQUEST_DATA_STREAM_DATA {
            target_system: 0,
            target_component: 0,
            req_stream_id: 0,
            req_message_rate: 10,
            start_stop: 1,
        })
    }
}
