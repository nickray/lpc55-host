//! Binary implementing the CLI in `cli.rs`

use core::convert::TryFrom;
use std::io::{self, Write as _};

use anyhow::{anyhow, Context as _};
use delog::hex_str;
use log::{info, trace};

mod cli;
mod logger;

fn main() {
    let args = cli::app().get_matches();
    if let Err(err) = try_main(args) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}

fn try_main(args: clap::ArgMatches<'_>) -> anyhow::Result<()> {

    logger::Logger::init().unwrap();

    match args.occurrences_of("v") {
        // 0 => log::set_max_level(log::LevelFilter::Error),
        0 => log::set_max_level(log::LevelFilter::Warn),
        1 => log::set_max_level(log::LevelFilter::Info),
        2 => log::set_max_level(log::LevelFilter::Debug),
        _ => log::set_max_level(log::LevelFilter::Trace),
    };

    let vid = u16::from_str_radix(args.value_of("VID").ok_or(anyhow!("Need a VID"))?.trim_start_matches("0x"), 16)
        .with_context(|| format!("Failed to parse VID"))?;
    let pid = u16::from_str_radix(args.value_of("PID").ok_or(anyhow!("Need a PID"))?.trim_start_matches("0x"), 16)
        .with_context(|| format!("Failed to parse PID"))?;

    trace!("using vid = {} pid = {}", vid, pid);

    if let Some(command) = args.subcommand_matches("http") {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
        let addr = command.value_of("ADDR").unwrap().to_string();
        let port = u16::from_str_radix(command.value_of("PORT").unwrap(), 10).unwrap();
        let http_config = lpc55::http::HttpConfig { addr, port, timeout_ms: 5000 };
        let server = lpc55::http::Server::new(&http_config, bootloader)?;
        server.run()?;
        return Ok(());
    }

    if let Some(_command) = args.subcommand_matches("info") {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid)?;
        bootloader.info();
        println!("{:#?}", bootloader.all_properties());
        return Ok(());
    }

    if args.subcommand_matches("reboot").is_some() {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid)?;
        bootloader.reboot();
    }

    if let Some(subcommand) = args.subcommand_matches("configure") {
        if let Some(subcommand) = subcommand.subcommand_matches("factory-settings") {
            let config_path = std::path::Path::new(subcommand.value_of("CONFIG").unwrap());
            let settings = std::fs::read_to_string(&config_path)?;
            let wrapped_settings: lpc55::protected_flash::WrappedFactorySettings = match config_path.extension() {
                Some(extension) => match extension {
                    os_str if os_str == "yaml" => {
                        serde_yaml::from_str(&settings)?
                    }
                    os_str if os_str == "toml" => {
                        toml::from_str(&settings)?
                    }
                    extension => todo!("extension {:?} not implemented", extension),
                }
                None => return Err(anyhow::anyhow!("no extension detected in path {:?}", &config_path)),
            };
            let settings = wrapped_settings.factory_settings;

            info!("settings: {:#?}", &settings);
            let settings = Vec::from(settings.to_bytes()?.as_ref());
            trace!("binary settings:\n{}", hex_str!(&settings, 4, sep: "\n"));


            if subcommand.value_of("OUTPUT").is_none() {
                let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid)?;
                bootloader.write_memory(lpc55::protected_flash::FACTORY_SETTINGS_ADDRESS, settings);
            } else {
                let output_name = subcommand.value_of("OUTPUT").unwrap();
                std::fs::write(&output_name, &settings).expect("Unable to write file");
                println!("outputing to file..");
            }
        }

        // TODO: https://github.com/NXPmicro/spsdk/blob/020a983e53769fe16cb9b49395d56f0201eccca6/spsdk/data/pfr/rules.json#L51-L61
        //
        // - ensure DICE key calc is disabled
        // - if secure-boot is on, at least one of RoT keys must be enabled

        if let Some(subcommand) = subcommand.subcommand_matches("customer-settings") {
            let config_path = std::path::Path::new(subcommand.value_of("CONFIG").unwrap());
            let settings = std::fs::read_to_string(&config_path)?;
            let wrapped_settings: lpc55::protected_flash::WrappedCustomerSettings = match config_path.extension() {
                Some(extension) => match extension {
                    os_str if os_str == "yaml" => {
                        serde_yaml::from_str(&settings)?
                    }
                    os_str if os_str == "toml" => {
                        toml::from_str(&settings)?
                    }
                    extension => todo!("extension {:?} not implemented", extension),
                }
                None => return Err(anyhow::anyhow!("no extension detected in path {:?}", &config_path)),
            };
            let settings = wrapped_settings.customer_settings;

            info!("settings: {:?}", &settings);
            let settings = Vec::from(settings.to_bytes()?.as_ref());
            info!("binary settings:\n{}", hex_str!(&settings, 4, sep: "\n"));

            if subcommand.value_of("OUTPUT").is_none() {
                let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid)?;
                bootloader.write_memory(lpc55::protected_flash::CUSTOMER_SETTINGS_SCRATCH_ADDRESS, settings);
            } else {
                let output_name = subcommand.value_of("OUTPUT").unwrap();
                std::fs::write(&output_name, &settings).expect("Unable to write file");
                println!("outputing to file..");
            }
        }

    }

    if let Some(subcommand) = args.subcommand_matches("keystore") {
        if subcommand.subcommand_matches("enroll-puf").is_some() {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
            bootloader.enroll_puf();
            return Ok(());
        }

        if subcommand.subcommand_matches("read").is_some() {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
            // let data = bootloader.read_memory(0x9_DE60, 3*512);

            let command = lpc55::types::Command::Keystore(
                lpc55::types::KeystoreOperation::ReadKeystore
            );
            let response = bootloader.protocol.call(&command).expect("success");

            let data = if let lpc55::types::Response::Data(data) = response {
                data
            } else {
                todo!();
            };

            let keystore = lpc55::protected_flash::Keystore::try_from(data.as_slice()).unwrap();
            println!("{}", serde_json::to_string(&keystore).unwrap());
            return Ok(());
        }

        if let Some(command) = subcommand.subcommand_matches("generate-key") {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
            let key = lpc55::types::Key::try_from(command.value_of("KEY").unwrap()).unwrap();
            let len: u32 = command.value_of("LENGTH").unwrap().parse()?;

            let command = lpc55::types::Command::Keystore(
                lpc55::types::KeystoreOperation::GenerateKey { key, len }
            );

            bootloader.protocol.call(&command).expect("success");
            return Ok(());
        }

        if let Some(command) = subcommand.subcommand_matches("set-key") {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
            let key = lpc55::types::Key::try_from(command.value_of("KEY").unwrap()).unwrap();
            let keydata_filename = command.value_of("KEYDATA_FILENAME").unwrap();
            let data = std::fs::read(keydata_filename)?;

            let command = lpc55::types::Command::Keystore(
                lpc55::types::KeystoreOperation::SetKey { key, data }
            );

            bootloader.protocol.call(&command).expect("success");
            return Ok(());
        }

        if subcommand.subcommand_matches("write-keys").is_some() {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();

            let command = lpc55::types::Command::Keystore(
                lpc55::types::KeystoreOperation::WriteNonVolatile { memory_id: 0 }
            );

            bootloader.protocol.call(&command).expect("success");
            return Ok(());
        }

        if subcommand.subcommand_matches("read-keys").is_some() {
            let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();

            let command = lpc55::types::Command::Keystore(
                lpc55::types::KeystoreOperation::ReadNonVolatile { memory_id: 0 }
            );

            bootloader.protocol.call(&command).expect("success");
            return Ok(());
        }

    }

    if let Some(command) = args.subcommand_matches("pfr") {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
        let data = bootloader.read_memory(0x9_DE00, 7*512);
        // let empty = data.iter().all(|&byte| byte == 0);
        // if empty {
        //     println!("PFR region is completely zeroed out");
        // } else {
        //     println!("PFR region is not completely zeroed out");
        // }
        let pfr = lpc55::protected_flash::ProtectedFlash::try_from(&data[..]).unwrap();
        // println!("PFR = {:#?}", &pfr);
        // println!("PFR = {:?}", &pfr);

        match command.value_of("FORMAT").unwrap() {
            "alt-native" => println!("{:#?}", &pfr),
            "native" => println!("{:?}", &pfr),
            "json" => println!("{}", serde_json::to_string(&pfr).unwrap()),
            "json-pretty" => println!("{}", serde_json::to_string_pretty(&pfr).unwrap()),
            "raw" => {
                if atty::is(atty::Stream::Stdout) {
                    panic!("don't dump binary data to terminal");
                } else {
                    io::stdout().write_all(&data).unwrap()
                }
            }
            "toml" => println!("{}", toml::to_string(&pfr).unwrap()),
            "yaml" => println!("{}", serde_yaml::to_string(&pfr).unwrap()),
            // "yaml-pretty" => println!("{}", serde_yaml::to_string_pretty(&pfr).unwrap()),
            _ => panic!(),
        }
        // let j = serde_json::to_string(&pfr).unwrap();
        // println!("{}", j);

        // println!("CFPA-scratch == CFPA-ping: {}", pfr.field.scratch == pfr.field.ping);
        // println!("CFPA-scratch == CFPA-pong: {}", pfr.field.scratch == pfr.field.pong);
        // println!("CFPA-ping == CFPA-pong: {}", pfr.field.ping == pfr.field.pong);
    }

    if let Some(command) = args.subcommand_matches("read-memory") {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
        let address = clap::value_t!(command.value_of("ADDRESS"), usize).unwrap();
        let length = clap::value_t!(command.value_of("LENGTH"), usize).unwrap();
        // let data = bootloader.read_memory_at_most_512(address, length);
        let data = bootloader.read_memory(address, length);

        if let Some(output_filename) = command.value_of("OUTPUT_FILE") {
            let mut file = std::fs::File::create(output_filename)?;
            use std::io::Write;
            file.write_all(&data)?;
            file.sync_all()?;
        } else {
            // lpc55::print_hex(data, 16);
            println!("{}", hex_str!(&data, 16));
        }
        return Ok(());
    }

    if let Some(command) = args.subcommand_matches("receive-sb-file") {
        let bootloader = lpc55::bootloader::Bootloader::try_new(vid, pid).unwrap();
        let filename = command.value_of("SB-FILE").unwrap();
        let image = std::fs::read(&filename)?;
        bootloader.receive_sb_file(image);
        return Ok(());
    }

    if let Some(command) = args.subcommand_matches("fingerprint-certificates") {
        use lpc55::pki::{Certificates, Pki};
        let config_filename = command.value_of("CONFIG").unwrap();
        let pki = Pki::try_from(config_filename)?;
        let certificates = Certificates::try_from_pki(&pki)?;
        let fingerprint = certificates.fingerprint();
        println!("{}", hex_str!(&fingerprint.0, 4));
    }

    if let Some(command) = args.subcommand_matches("sign-fw") {
        use lpc55::{
            secure_binary::Config,
            signed_binary::ImageSigningRequest,
        };
        let config_filename = command.value_of("CONFIG").unwrap();
        let config = Config::try_from(config_filename)?;
        // let _signed_image = lpc55::signed_binary::sign(&config)?;
        let signing_request = ImageSigningRequest::try_from(&config)?;
        let signed_image = signing_request.sign();
        std::fs::write(&config.firmware.signed_image, &signed_image.0)?;

        //////////////////////////////////////////////////////
        //
        // TODO NEXT: use ImageSigningRequest
        // THEN: make sure uri-certificates in Config work
        //
        //////////////////////////////////////////////////////
    }

    if let Some(command) = args.subcommand_matches("assemble-sb") {
        use lpc55::secure_binary::{SignedSb21File, UnsignedSb21File};
        let config_filename = command.value_of("CONFIG").unwrap();
        let config = lpc55::secure_binary::Config::try_from(config_filename)?;
        let unsigned_image = UnsignedSb21File::try_assemble_from(&config)?;
        let signing_key = lpc55::pki::SigningKey::try_from_uri(config.pki.signing_key.as_ref())?;
        // dbg!(&signing_key);
        let signed_image: SignedSb21File = unsigned_image.sign(&signing_key);
        let signed_image_bytes = signed_image.to_bytes();
        std::fs::write(&config.firmware.secure_boot_image, &signed_image_bytes)?;
        // dbg!(signed_image_bytes.len());
    }

    if let Some(subcommand) = args.subcommand_matches("sb") {
        if let Some(command) = subcommand.subcommand_matches("show") {
            let filename = command.value_of("FILE").unwrap();
            lpc55::secure_binary::show(filename)?;
        }
    }

    Ok(())
}

////////
        // bootloader.info();

        // println!("current version: {}", bootloader.properties().current_version().unwrap());
        // println!("target version: {}", bootloader.properties().target_version().unwrap());
        // println!("available commands: {:?}", bootloader.properties().available_commands().unwrap());
        // println!("available peripherals: {:?}", bootloader.properties().available_peripherals().unwrap());
        // println!("PFR (protected flash region) keystore update options: {:?}",
        //     bootloader.properties().pfr_keystore_update_option().unwrap());
        // println!("RAM start address: 0x{:08X}", bootloader.properties().ram_start_address().unwrap());
        // println!("RAM size: {}", bootloader.properties().ram_size().unwrap());
        // println!("flash start address: 0x{:08X}", bootloader.properties().flash_start_address().unwrap());
        // println!("flash size: {}", bootloader.properties().flash_size().unwrap());
        // println!("flash page size: {}", bootloader.properties().flash_page_size().unwrap());
        // println!("flash sector size: {}", bootloader.properties().flash_sector_size().unwrap());
        // println!("verify writes: {}", bootloader.properties().verify_writes().unwrap());
        // println!("flash locked: {}", bootloader.properties().flash_locked().unwrap());
        // println!("max packet size: {}", bootloader.properties().max_packet_size().unwrap());
        // println!("device UUID: 0x{:16X}", bootloader.properties().device_uuid().unwrap());
        // println!("system UUID: 0x{:08X}", bootloader.properties().system_uuid().unwrap());
        // println!("CRC check status: {:?}", bootloader.properties().crc_check_status().unwrap());
        // println!("reserved regions:");
        // for (left, right) in bootloader.properties().reserved_regions().unwrap().iter() {
        //     println!("  0x{:08X} - 0x{:08X} ({:.2} KB)", left, right, ((*right + 1) as f64 - *left as f64)/1024.);
        // }
        // println!("IRQ notification PIN: {:?}", bootloader.properties().irq_notification_pin().unwrap());

