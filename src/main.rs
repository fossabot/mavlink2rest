use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use actix_web::http::StatusCode;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use chrono;
use chrono::offset::TimeZone;
use clap;
use serde_derive::Deserialize;
use serde_json::json;

mod message_information;
use message_information::MessageInformation;

use lazy_static::lazy_static;
lazy_static! {
    static ref MESSAGES: std::sync::Arc<Mutex<serde_json::value::Value>> = {
        // Create an empty map with the main key as mavlink
        return Arc::new(Mutex::new(json!({"mavlink":{}})));
    };
}

fn main() {
    let matches = clap::App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("MAVLink to REST API!.")
        .author(env!("CARGO_PKG_AUTHORS"))
        .arg(
            clap::Arg::with_name("connect")
                .short("c")
                .long("connect")
                .value_name("TYPE:<IP/SERIAL>:<PORT/BAUDRATE>")
                .help("Sets the mavlink connection string")
                .takes_value(true)
                .default_value("udpin:0.0.0.0:14550"),
        )
        .arg(
            clap::Arg::with_name("server")
                .short("s")
                .long("server")
                .value_name("IP:PORT")
                .help("Sets the IP and port that the rest server will be provided")
                .takes_value(true)
                .default_value("0.0.0.0:8088"),
        )
        .arg(
            clap::Arg::with_name("verbose")
                .short("v")
                .long("verbose")
                .help("Be verbose")
                .takes_value(false),
        )
        .get_matches();

    let verbose = matches.is_present("verbose");
    let server_string = matches.value_of("server").unwrap();
    let connection_string = matches.value_of("connect").unwrap();

    println!("MAVLink connection string: {}", connection_string);
    println!("REST API address: {}", server_string);

    let mavconn = mavlink::connect(connection_string).unwrap();

    let vehicle = Arc::new(mavconn);
    let _ = vehicle.send_default(&request_stream());

    thread::spawn({
        let vehicle = vehicle.clone();
        move || loop {
            let res = vehicle.send_default(&heartbeat_message());
            if res.is_err() {
                println!("Failed to send heartbeat");
            }
            thread::sleep(Duration::from_secs(1));
        }
    });

    thread::spawn({
        let vehicle = vehicle.clone();
        let messages_ref = Arc::clone(&MESSAGES);

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
                        if verbose {
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
        }
    });

    HttpServer::new(|| {
        App::new()
            .route("/", web::get().to(root_page))
            .route("/mavlink|/mavlink/*", web::get().to(mavlink_page))
            .route("/mavlink", web::post().to(malink_post))
    })
    .bind(server_string)
    .unwrap()
    .run()
    .unwrap();
}

fn root_page(_req: HttpRequest) -> impl Responder {
    let messages = Arc::clone(&MESSAGES);
    let messages = messages.lock().unwrap();
    let mut html_list_content = String::new();
    let now = chrono::Local::now();
    for key in messages["mavlink"].as_object().unwrap().keys() {
        let frequency = messages["mavlink"][&key]["message_information"]["frequency"]
            .as_f64()
            .unwrap_or(0.0);
        let last_time = now
            - chrono::Local
                .datetime_from_str(
                    &messages["mavlink"][&key]["message_information"]["time"]["last_message"]
                        .to_string(),
                    "\"%+\"",
                )
                .unwrap_or(now);
        html_list_content = format!(
            "{0} <li> <a href=\"mavlink/{1}\">mavlink/{1}</a> ({2:.2}Hz - last update {3:#?}s ago) </li>",
            html_list_content,
            key,
            frequency,
            last_time.num_milliseconds() as f64/1e3
        );
    }
    // Remove guard after clone
    std::mem::drop(messages);

    let html_list = format!("<ul> {} </ul>", html_list_content);

    let html = format!(
        "{} - {} - {}<br>By: {}<br>
        Check the <a href=\"\\mavlink\">mavlink path</a> for the data<br>
        You can also check nested paths: <a href=\"mavlink/HEARTBEAT/mavtype/type\">mavlink/HEARTBEAT/mavtype/type</a><br>
        <br>
        List of available paths:
        {}
        ",
        env!("CARGO_PKG_NAME"),
        env!("VERGEN_SEMVER"),
        env!("VERGEN_BUILD_DATE"),
        env!("CARGO_PKG_AUTHORS"),
        html_list,
    );
    HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(html)
}

#[derive(Deserialize, Debug, Default)]
pub struct JsonConfiguration {
    pretty: Option<bool>,
}

fn mavlink_page(req: HttpRequest) -> impl Responder {
    let query = web::Query::<JsonConfiguration>::from_query(req.query_string())
        .unwrap_or(web::Query(Default::default()));

    let url_path = req.path().to_string();
    let messages = Arc::clone(&MESSAGES);
    let messages = messages.lock().unwrap();
    let final_result = (*messages).pointer(&url_path);

    if final_result.is_none() {
        return "No valid path".to_string();
    }

    let final_result = final_result.unwrap().clone();
    std::mem::drop(messages); // Remove guard after clone

    if !query.pretty.is_none() && query.pretty.unwrap() {
        return serde_json::to_string_pretty(&final_result)
            .unwrap()
            .to_string();
    }

    return serde_json::to_string(&final_result).unwrap().to_string();
}

#[derive(Debug, Deserialize)]
pub struct MyObj {
    pub header: mavlink::MavHeader,
    pub message: mavlink::common::MavMessage,
}

pub fn malink_post(req: web::Json<MyObj>) -> impl Responder {
    //let j = json!({"chan10_raw":0,"chan11_raw":0,"chan12_raw":0,"chan13_raw":0,"chan14_raw":0,"chan15_raw":0,"chan16_raw":0,"chan17_raw":0,"chan18_raw":0,"chan1_raw":1500,"chan2_raw":1500,"chan3_raw":1000,"chan4_raw":1500,"chan5_raw":1800,"chan6_raw":1000,"chan7_raw":1000,"chan8_raw":1800,"chan9_raw":0,"chancount":16,"rssi":0,"time_boot_ms":3838,"type":"RC_CHANNELS"});
    println!("> {:#?}", &req);
    //let v = serde_json::from_value::<mavlink::common::MavMessage>(req.into_inner());
    let content = &req.into_inner();
    format!("> {:#?}\n > {:#?}", &content.header, &content.message)
}

pub fn heartbeat_message() -> mavlink::common::MavMessage {
    mavlink::common::MavMessage::HEARTBEAT(mavlink::common::HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: mavlink::common::MavType::MAV_TYPE_QUADROTOR,
        autopilot: mavlink::common::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
        base_mode: mavlink::common::MavModeFlag::empty(),
        system_status: mavlink::common::MavState::MAV_STATE_STANDBY,
        mavlink_version: 0x3,
    })
}

pub fn request_stream() -> mavlink::common::MavMessage {
    mavlink::common::MavMessage::REQUEST_DATA_STREAM(mavlink::common::REQUEST_DATA_STREAM_DATA {
        target_system: 0,
        target_component: 0,
        req_stream_id: 0,
        req_message_rate: 10,
        start_stop: 1,
    })
}
