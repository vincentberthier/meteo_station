CREATE TABLE IF NOT EXISTS samples (
  bucket_ts     INTEGER PRIMARY KEY,   -- unix epoch seconds, floored to the minute
  temp_min REAL, temp_max REAL, temp_avg REAL,
  pressure_min REAL, pressure_max REAL, pressure_avg REAL,
  humidity_min REAL, humidity_max REAL, humidity_avg REAL,
  sky_min REAL, sky_max REAL, sky_avg REAL,
  lux_min REAL, lux_max REAL, lux_avg REAL,
  wind_min REAL, wind_max REAL, wind_avg REAL,   -- wind_max = gust
  wind_dir_avg REAL,                              -- vector-mean heading, degrees
  rain_avg REAL, rain_max REAL,
  battery_avg REAL,
  solar_mv_avg REAL, solar_ma_avg REAL, batt_mv_avg REAL, load_ma_avg REAL,
  sample_count INTEGER NOT NULL
);
