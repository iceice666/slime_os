//! The ACPI-described i8042 PS/2 keyboard for the PC-class platform.
//!
//! Owns bring-up and raw byte delivery only: bounded controller commands, the
//! self-test sequence, and interrupt routing through the I/O APIC. Decoded
//! events, the queue, and the blocked-reader waiter live in the neutral
//! [`crate::input`] driver, which this feeds through `feed_scancode`.

use crate::arch::x86_64::i8042;
use crate::input::{InputError, InputInitReport, InputPath, InputStage};
use crate::platform::acpi::MadtInfo;
use crate::serial_println;
use crate::time::apic::RouteError;

pub fn init(madt: &MadtInfo, i8042_present: bool) -> Result<(), InputError> {
    init_with_report(madt, i8042_present, false).result()
}

pub fn init_with_report(
    madt: &MadtInfo,
    i8042_present: bool,
    usb_controller_present: bool,
) -> InputInitReport {
    let mut report = InputInitReport::new(if i8042_present {
        InputPath::I8042
    } else if usb_controller_present {
        InputPath::UsbHid
    } else {
        InputPath::FirmwareOther
    });
    report.push(InputStage::FirmwareHint, None);
    if !i8042_present {
        report.push(
            InputStage::Failed,
            Some(InputError::ControllerNotImplemented),
        );
        return report;
    }

    i8042::command_immediate(0xad);
    i8042::command_immediate(0xa7);
    report.push(InputStage::PortsDisabled, None);
    i8042::drain_output();
    report.push(InputStage::OutputDrained, None);

    let mut config = match read_controller_config() {
        Ok(config) => config,
        Err(error) => {
            report.push(InputStage::ConfigRead, Some(error));
            return report;
        }
    };
    config &= !0x43;
    if let Err(error) = write_controller_config(config) {
        report.push(InputStage::ConfigRead, Some(error));
        return report;
    }
    report.push(InputStage::ConfigRead, None);

    if let Err(error) = command(0xaa) {
        report.push(InputStage::ControllerSelfTest, Some(error));
        return report;
    }
    let self_test = match read_data() {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::ControllerSelfTest, Some(error));
            return report;
        }
    };
    if self_test != 0x55 {
        report.push(
            InputStage::ControllerSelfTest,
            Some(InputError::ControllerSelfTestFailed(self_test)),
        );
        return report;
    }
    report.push(InputStage::ControllerSelfTest, None);

    let port_result = (|| {
        command(0xae)?;
        config = read_controller_config()?;
        config |= 0x41;
        config &= !0x10;
        write_controller_config(config)
    })();
    if let Err(error) = port_result {
        report.push(InputStage::FirstPortEnabled, Some(error));
        return report;
    }
    report.push(InputStage::FirstPortEnabled, None);

    let reset_ack = (|| {
        write_data(0xff)?;
        read_data()
    })();
    let ack = match reset_ack {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::KeyboardResetAck, Some(error));
            return report;
        }
    };
    if ack != 0xfa {
        report.push(
            InputStage::KeyboardResetAck,
            Some(InputError::KeyboardResetFailed(ack)),
        );
        return report;
    }
    report.push(InputStage::KeyboardResetAck, None);

    let reset = match read_data() {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::KeyboardSelfTest, Some(error));
            return report;
        }
    };
    if reset != 0xaa {
        report.push(
            InputStage::KeyboardSelfTest,
            Some(InputError::KeyboardResetFailed(reset)),
        );
        return report;
    }
    report.push(InputStage::KeyboardSelfTest, None);

    let enable_ack = (|| {
        write_data(0xf4)?;
        read_data()
    })();
    let enable = match enable_ack {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::ScanningEnabled, Some(error));
            return report;
        }
    };
    if enable != 0xfa {
        report.push(
            InputStage::ScanningEnabled,
            Some(InputError::KeyboardResetFailed(enable)),
        );
        return report;
    }
    report.push(InputStage::ScanningEnabled, None);

    if let Err(error) =
        crate::time::apic::route_external_irq(madt, 1, crate::interrupts::KEYBOARD_VECTOR)
    {
        let error = match error {
            RouteError::MissingIoApic => InputError::RouteMissingIoApic,
            RouteError::GsiOutOfRange => InputError::RouteGsiOutOfRange,
            RouteError::Map(_) => InputError::RouteMapFailed,
        };
        report.push(InputStage::InterruptRouted, Some(error));
        return report;
    }
    report.push(InputStage::InterruptRouted, None);
    crate::input::set_present();
    report.push(InputStage::Online, None);
    serial_println!("[input] i8042 keyboard online");
    report
}

pub(crate) fn on_interrupt() {
    if i8042::output_full() {
        let scancode = i8042::read_data_port();
        crate::input::feed_scancode(scancode);
    }
}

impl From<i8042::ControllerTimeout> for InputError {
    fn from(_: i8042::ControllerTimeout) -> Self {
        Self::ControllerTimeout
    }
}

fn read_controller_config() -> Result<u8, InputError> {
    command(0x20)?;
    read_data()
}

fn write_controller_config(config: u8) -> Result<(), InputError> {
    command(0x60)?;
    write_data(config)
}

fn command(value: u8) -> Result<(), InputError> {
    Ok(i8042::command(value)?)
}

fn write_data(value: u8) -> Result<(), InputError> {
    Ok(i8042::write_data(value)?)
}

fn read_data() -> Result<u8, InputError> {
    Ok(i8042::read_data()?)
}
