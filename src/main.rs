use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use esp_idf_hal::{delay::FreeRtos, gpio::PinDriver, peripherals::Peripherals};
use log::*;
use anyhow::{Result};
use std::{net::{UdpSocket, SocketAddr}, time::{SystemTime, Duration, Instant}, thread};
use smol;
use serde_json::json;
use esp_idf_sys::{
    esp, gpio_config, gpio_config_t, gpio_install_isr_service, gpio_int_type_t_GPIO_INTR_POSEDGE, gpio_int_type_t_GPIO_INTR_NEGEDGE,
    gpio_isr_handler_add, gpio_mode_t_GPIO_MODE_INPUT, xQueueGenericCreate, xQueueGiveFromISR,
    xQueueReceive, QueueHandle_t, ESP_INTR_FLAG_IRAM,
};
use std::ptr;

pub mod wifi;

use crate::wifi::wifi;

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
    #[default("base")]
    area: &'static str,
    #[default("Flow Area 0.1")]
    flow_name: &'static str,
    #[default("192.168.0.104")]
    target_ip: &'static str,
    #[default(33001)]
    target_port: u16,
    #[default(29000)]
    outbound_port: u16,
    #[default(29001)]
    inbound_port: u16,
}

// This `static mut` holds the queue handle we are going to get from `xQueueGenericCreate`.
// This is unsafe, but we are careful not to enable our GPIO interrupt handler until after this value has been initialised, and then never modify it again
static mut EVENT_QUEUE: Option<QueueHandle_t> = None;

// #[link_section = ".iram0.text"] // This is required to place the function in IRAM, but the linker is throwing a fit. Should work without it, but it will be slower and potentially unreliable
unsafe extern "C" fn button_interrupt(_: *mut core::ffi::c_void) {
    xQueueGiveFromISR(EVENT_QUEUE.unwrap(), std::ptr::null_mut());
}

fn main() -> Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();
    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let gpio_num = 12;

    // Configures the button
    let io_conf = gpio_config_t {
        pin_bit_mask: 1 << gpio_num,
        mode: gpio_mode_t_GPIO_MODE_INPUT,
        pull_up_en: true.into(),
        pull_down_en: false.into(),
        intr_type: gpio_int_type_t_GPIO_INTR_NEGEDGE, // Positive edge trigger = button down
    };

    // Queue configurations
    const QUEUE_TYPE_BASE: u8 = 0;
    const ITEM_SIZE: u32 = 0; // We're not posting any actual data, just notifying
    const QUEUE_SIZE: u32 = 1;

    unsafe {
        // Writes the button configuration to the registers
        esp!(gpio_config(&io_conf))?;

        // Installs the generic GPIO interrupt handler
        esp!(gpio_install_isr_service(ESP_INTR_FLAG_IRAM as i32))?;

        // Instantiates the event queue
        EVENT_QUEUE = Some(xQueueGenericCreate(QUEUE_SIZE, ITEM_SIZE, QUEUE_TYPE_BASE));

        // Registers our function with the generic GPIO interrupt handler we installed earlier.
        esp!(gpio_isr_handler_add(
            gpio_num,
            Some(button_interrupt),
            std::ptr::null_mut()
        ))?;
    }

    // !!! pins that should not be used on ESP32: 6-11, 16-17
    // those are reserved for SPI and trying to use them will result in a watchdog timer reset
    let peripherals = Peripherals::take().unwrap();

    let sysloop = EspSystemEventLoop::take()?;

    // let input_pin = PinDriver::input(peripherals.pins.gpio13).unwrap();
    let mut output_pin = PinDriver::output(peripherals.pins.gpio17).unwrap();
    output_pin.set_low().unwrap();

    // The constant `CONFIG` is auto-generated by `toml_config`.
    let config = CONFIG;

    info!("SSID: {}", config.wifi_ssid);
    info!("PSK: {}", config.wifi_psk);
    
    // Connect to the Wi-Fi network
    let wifi_interface = wifi(
        config.wifi_ssid,
        config.wifi_psk,
        peripherals.modem,
        sysloop,
    ).expect("Couldn't connect to WiFi!");

    info!("IP (sta): {}", wifi_interface.sta_netif().get_ip_info()?.ip);
    info!("IP (ap): {}", wifi_interface.ap_netif().get_ip_info()?.ip);

    let socket_inbound_address = SocketAddr::from(([0, 0, 0, 0], config.inbound_port));
    let socket_outbound_address = SocketAddr::from(([0, 0, 0, 0], config.outbound_port));
    info!("socket address created");
    let outbound_socket = UdpSocket::bind(socket_outbound_address)?;
    let inbound_socket = UdpSocket::bind(socket_inbound_address)?;
    info!("socket bound");

    let mut target: SocketAddr = SocketAddr::from(format!("{}:{}", "192.168.178.125", "33001").parse::<SocketAddr>().expect("No valid target address given. Use format: <ip>:<port>"));
    let timeout = Duration::from_millis(10);
    inbound_socket.set_read_timeout(timeout.into()).expect("Couldn't set socket timeout");


    let mut timer_generate_data = Instant::now();
    let mut timer_check_incoming = timer_generate_data.clone();

    let mut buf = [0; 1024];

    thread::Builder::new().stack_size(32768).spawn(move || {

        loop {

            if timer_check_incoming.elapsed().as_millis() > 10 {

                // check socket for incoming data
                if let Ok((message_length, src)) = inbound_socket.recv_from(&mut buf) {
                    // convert to string
                    let message = String::from_utf8(buf[..message_length].into()).expect("Couldn't convert to String");
                    info!("Received data from {}: {}", src, message);

                    // parse json
                    let json: serde_json::Value = serde_json::from_str(&message).expect("Couldn't parse JSON");
                    if let Some(message_type) = json["type"].as_str() {
                        match message_type {
                            "updateTarget" => {

                                // take 10k part from the new target port and fill the rest with the old one
                                let new_target_port_base = json["target_port_base"].as_u64().expect("No target base port given") as u16;
                                let new_target_port = new_target_port_base + (config.target_port % 10000);
                                info!("New target port: {}", new_target_port);

                                let new_target_address_string = format!("{}:{}", json["target"].as_str().expect("No target ip given"), new_target_port);
                                let new_target_address = new_target_address_string.parse::<SocketAddr>().expect(format!("Target not updated because target address was invalid: {}", new_target_address_string).as_str());
                                target = new_target_address;
                                
                                // acknowledge
                                let json = json!({
                                    "type": "updateTarget",
                                    "success": true,
                                });
                                info!("Sending ACK to {}: {}", src, json.to_string());
                                // send 10 times to "make sure" it arrives
                                for _ in 0..10 {
                                    outbound_socket.send_to(json.to_string().as_bytes(), src).expect("Couldn't send ACK");
                                }
                            },
                            "udpPing" => {
                                let start = SystemTime::now();
                                let time = start.duration_since(std::time::UNIX_EPOCH).expect("Couldn't get system time");
                                let return_buf = (time.as_micros() as u64).to_be_bytes();
                                let return_address = json["replyTo"].as_str().unwrap().parse::<SocketAddr>().expect("No return address given");

                                // send current system time back to sender
                                outbound_socket.send_to(&return_buf, &return_address).unwrap();
                                info!("Sent UDP ping response to {}", return_address);
                            },
                            _ => {}
                        }
                    } 
        
                } else {
                    // no data received
                    // info!("No data received")
                }

                // reset timer
                timer_check_incoming = Instant::now();
            }

            if timer_generate_data.elapsed().as_millis() > 2000 {

                unsafe {
                    // Maximum delay
                    const QUEUE_WAIT_TICKS: u32 = 0;

                    // Reads the event item out of the queue
                    let res = xQueueReceive(EVENT_QUEUE.unwrap(), ptr::null_mut(), QUEUE_WAIT_TICKS);

                    // If the event has the value 0, nothing happens. if it has a different value, the button was pressed.
                    match res {
                        1 => {
                            warn!("Button pressed!");
                            // let data = generate_input_data();
                            output_pin.set_high().unwrap();

                            let data = "test";

                            let json = json!({
                                "message": data.to_string(),
                                "meta": {
                                    "flow_name": config.flow_name,
                                    "execution_area": config.area
                                }
                            });
                            
                            // info!("Sending data to {}: {}", target, data);
                            info!("Sending data to {}: {}", target, data);
                            outbound_socket.send_to(json.to_string().as_bytes(), target).expect("Couldn't send data");
                            // outbound_socket.send_to(data.to_string().as_bytes(), target).expect("Couldn't send data");

                            FreeRtos::delay_ms(100);
                            output_pin.set_low().unwrap();
                        },
                        0 => {},
                        _ => {},
                    };
                }

                // reset timer
                timer_generate_data = Instant::now();
            }
            
            // if timer_generate_data.elapsed().as_millis() > 1000 {

            //     // let data = generate_input_data();
            //     let data = "test";

            //     let json = json!({
            //         "message": data.to_string(),
            //         "meta": {
            //             "flow_name": config.flow_name,
            //             "execution_area": config.area
            //         }
            //     });
                
            //     // info!("Sending data to {}: {}", target, data);
            //     info!("Sending data to {}: {}", target, data);
            //     outbound_socket.send_to(&json.to_string().as_bytes(), target).expect("Couldn't send data");
            //     // outbound_socket.send_to(data.to_string().as_bytes(), target).expect("Couldn't send data");

            //     // reset timer
            //     timer_generate_data = Instant::now();
            // }
            
        }

    })?.join().expect("Couldn't join thread");

    Ok(())

}
