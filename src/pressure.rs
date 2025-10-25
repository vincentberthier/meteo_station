use defmt::{debug, error, info, trace, warn};
use embassy_rp::{
    i2c::{Async, I2c},
    peripherals::I2C0,
};
use embassy_time::{Duration, Timer};

// BMP388 Register addresses
const BMP388_CHIP_ID_REG: u8 = 0x00;
const BMP388_PWR_CTRL: u8 = 0x1B;
const BMP388_PRESS_MSB: u8 = 0x04;
const BMP388_TEMP_MSB: u8 = 0x07;
const BMP388_CHIP_ID: u8 = 0x50;

// Calibration registers start at 0x31
const BMP388_CALIB_DATA: u8 = 0x31;

// Structure to hold calibration data (converted to float for compensation)
#[derive(Debug)]
struct CalibData {
    par_t1: f32,
    par_t2: f32,
    par_t3: f32,
    par_p1: f32,
    par_p2: f32,
    par_p3: f32,
    par_p4: f32,
    par_p5: f32,
    par_p6: f32,
    par_p7: f32,
    par_p8: f32,
    par_p9: f32,
    par_p10: f32,
    par_p11: f32,
    t_lin: f32, // Stores compensated temperature for pressure calculation
}

impl CalibData {
    fn from_raw_bytes(data: &[u8; 21]) -> Self {
        // Extract raw values from bytes (see datasheet Table 23)
        let nvm_par_t1 = u16::from_le_bytes([data[0], data[1]]);
        let nvm_par_t2 = u16::from_le_bytes([data[2], data[3]]);
        let nvm_par_t3 = i8::from_le_bytes([data[4]]);

        let nvm_par_p1 = i16::from_le_bytes([data[5], data[6]]);
        let nvm_par_p2 = i16::from_le_bytes([data[7], data[8]]);
        let nvm_par_p3 = i8::from_le_bytes([data[9]]);
        let nvm_par_p4 = i8::from_le_bytes([data[10]]);
        let nvm_par_p5 = u16::from_le_bytes([data[11], data[12]]);
        let nvm_par_p6 = u16::from_le_bytes([data[13], data[14]]);
        let nvm_par_p7 = i8::from_le_bytes([data[15]]);
        let nvm_par_p8 = i8::from_le_bytes([data[16]]);
        let nvm_par_p9 = i16::from_le_bytes([data[17], data[18]]);
        let nvm_par_p10 = i8::from_le_bytes([data[19]]);
        let nvm_par_p11 = i8::from_le_bytes([data[20]]);

        // Convert to floating point using formulas from datasheet section 9.1
        Self {
            par_t1: (nvm_par_t1 as f32) * 256.0,
            par_t2: (nvm_par_t2 as f32) / 1073741824.0, // 2^30
            par_t3: (nvm_par_t3 as f32) / 281474976710656.0, // 2^48

            par_p1: ((nvm_par_p1 as f32) - 16384.0) / 1048576.0, // (x - 2^14) / 2^20
            par_p2: ((nvm_par_p2 as f32) - 16384.0) / 536870912.0, // (x - 2^14) / 2^29
            par_p3: (nvm_par_p3 as f32) / 4294967296.0,          // 2^32
            par_p4: (nvm_par_p4 as f32) / 137438953472.0,        // 2^37
            par_p5: (nvm_par_p5 as f32) / 0.125,                 // 2^-3
            par_p6: (nvm_par_p6 as f32) / 64.0,                  // 2^6
            par_p7: (nvm_par_p7 as f32) / 256.0,                 // 2^8
            par_p8: (nvm_par_p8 as f32) / 32768.0,               // 2^15
            par_p9: (nvm_par_p9 as f32) / 281474976710656.0,     // 2^48
            par_p10: (nvm_par_p10 as f32) / 281474976710656.0,   // 2^48
            par_p11: (nvm_par_p11 as f32) / 36893488147419103232.0, // 2^65
            t_lin: 0.0,
        }
    }

    // Temperature compensation (datasheet section 9.2)
    fn compensate_temperature(&mut self, uncomp_temp: u32) -> f32 {
        let partial_data1 = uncomp_temp as f32 - self.par_t1;
        let partial_data2 = partial_data1 * self.par_t2;

        // Store t_lin for pressure compensation
        self.t_lin = partial_data2 + (partial_data1 * partial_data1) * self.par_t3;

        self.t_lin // Returns temperature in °C
    }

    // Pressure compensation (datasheet section 9.3)
    fn compensate_pressure(&self, uncomp_press: u32) -> f32 {
        let partial_data1 = self.par_p6 * self.t_lin;
        let partial_data2 = self.par_p7 * (self.t_lin * self.t_lin);
        let partial_data3 = self.par_p8 * (self.t_lin * self.t_lin * self.t_lin);
        let partial_out1 = self.par_p5 + partial_data1 + partial_data2 + partial_data3;

        let partial_data1 = self.par_p2 * self.t_lin;
        let partial_data2 = self.par_p3 * (self.t_lin * self.t_lin);
        let partial_data3 = self.par_p4 * (self.t_lin * self.t_lin * self.t_lin);
        let partial_out2 =
            (uncomp_press as f32) * (self.par_p1 + partial_data1 + partial_data2 + partial_data3);

        let partial_data1 = (uncomp_press as f32) * (uncomp_press as f32);
        let partial_data2 = self.par_p9 + self.par_p10 * self.t_lin;
        let partial_data3 = partial_data1 * partial_data2;
        let partial_data4 = partial_data3
            + ((uncomp_press as f32) * (uncomp_press as f32) * (uncomp_press as f32))
                * self.par_p11;

        partial_out1 + partial_out2 + partial_data4 // Returns pressure in Pa
    }
}

#[embassy_executor::task]
pub async fn read_barometer(mut i2c: I2c<'static, I2C0, Async>) {
    let address = 0x77u8; // Try 0x77 if this doesn't work
    debug!("Setting up barometer");

    // Verify chip ID
    let mut chip_id = [0u8; 1];
    match i2c
        .write_read_async(address, [BMP388_CHIP_ID_REG], &mut chip_id)
        .await
    {
        Ok(_) => {
            if chip_id[0] == BMP388_CHIP_ID {
                trace!("BMP388 found! Chip ID: 0x{:02X}", chip_id[0]);
            } else {
                warn!(
                    "Wrong chip ID: 0x{:02X}, expected 0x{:02X}",
                    chip_id[0], BMP388_CHIP_ID
                );
                return;
            }
        }
        Err(e) => {
            error!("Failed to read chip ID: {:?}", e);
            error!("Check wiring and I2C address (try 0x77 if using 0x76)");
            return;
        }
    }

    // Enable pressure and temperature sensors in normal mode
    // PWR_CTRL: bit 0 = press_en, bit 1 = temp_en, bits 4-5 = mode (11 = normal)
    if let Err(e) = i2c.write_async(address, [BMP388_PWR_CTRL, 0x33]).await {
        error!("Failed to enable sensors: {:?}", e);
        return;
    }

    Timer::after(Duration::from_millis(100)).await;
    info!("BMP388 initialized successfully!");

    // Read calibration data
    let mut calib_raw = [0u8; 21];
    if let Err(e) = i2c
        .write_read_async(address, [BMP388_CALIB_DATA], &mut calib_raw)
        .await
    {
        error!("Failed to read calibration data: {:?}", e);
        return;
    }

    let mut calib = CalibData::from_raw_bytes(&calib_raw);
    debug!("Calibration data loaded");

    // Main reading loop
    loop {
        // Read raw pressure data (3 bytes)
        let mut press_data = [0u8; 3];
        if (i2c
            .write_read_async(address, [BMP388_PRESS_MSB], &mut press_data)
            .await)
            .is_ok()
        {
            let press_raw = ((press_data[2] as u32) << 16)
                | ((press_data[1] as u32) << 8)
                | (press_data[0] as u32);

            // Read raw temperature data (3 bytes)
            let mut temp_data = [0u8; 3];
            if (i2c
                .write_read_async(address, [BMP388_TEMP_MSB], &mut temp_data)
                .await)
                .is_ok()
            {
                let temp_raw = ((temp_data[2] as u32) << 16)
                    | ((temp_data[1] as u32) << 8)
                    | (temp_data[0] as u32);

                // Compensate values
                let temperature = calib.compensate_temperature(temp_raw);
                let pressure = calib.compensate_pressure(press_raw);

                info!(
                    "Temperature: {}°C, Pressure: {} Pa ({} hPa)",
                    trunc2(temperature),
                    trunc2(pressure),
                    trunc2(pressure / 100.0)
                );
            }
        }

        Timer::after(Duration::from_secs(1)).await;
    }
}

fn trunc2(v: f32) -> f32 {
    let scaled = v * 100.0;
    let scaled_i = scaled as i32; // cast truncates toward zero
    scaled_i as f32 / 100.0
}
