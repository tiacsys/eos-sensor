#![no_std]

pub use embassy_stm32 as hal;
use hal::{
    eth::{
        Ethernet,
        generic_smi::GenericSMI,
    },
    i2c::I2c,
    peripherals::{
        ETH,
        RNG,
    },
    mode::Blocking,
    rng::Rng,
};

use lsm9ds1::{LSM9DS1, interface::I2cInterface};

pub type Sensor = LSM9DS1<I2cInterface<I2c<'static, Blocking>>>;
pub type NetworkDevice = Ethernet<'static, ETH, GenericSMI>;
pub type RngDevice = Rng<'static, RNG>;

#[cfg(feature = "type-checks")]
mod type_checks {
    #![allow(dead_code)]
    use rand_core::RngCore;
    use embassy_net_driver::Driver;

    fn implements_rngcore<T: RngCore>() {}
    fn implements_driver<T: Driver>() {}

    fn rng_implements_rngcore() {
        implements_rngcore::<super::RngDevice>();
    }

    fn network_device_implements_driver() {
        implements_driver::<super::NetworkDevice>();
    }
}
