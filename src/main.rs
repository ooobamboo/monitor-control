use std::fs::read_dir;
use std::num::ParseIntError;

use clap::Parser;
use ddc::Ddc;
use eyre::{bail, eyre, Result};

fn parse_feature_code(input: &str) -> Result<u8, ParseIntError> {
    if let Some(s) = input.strip_prefix("0x") {
        u8::from_str_radix(s, 16)
    } else if let Some(s) = input.strip_suffix(&['h', 'H']) {
        u8::from_str_radix(s, 16)
    } else {
        input.parse()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueType {
    Absolute,
    Relative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeltaType {
    Direct,
    Delta,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sign {
    Plus,
    Minus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ValueSpec {
    value: u16,
    value_type: ValueType,
    delta_type: DeltaType,
    sign: Sign,
}

fn parse_value(input: &str) -> Result<ValueSpec> {
    let mut value_type = ValueType::Absolute;
    let mut delta_type = DeltaType::Direct;
    let mut sign = Sign::Plus;
    let mut s = input;

    if s.is_empty() {
        bail!("value expression must not be empty");
    }

    if let Some(rest) = s.strip_prefix('+') {
        sign = Sign::Plus;
        delta_type = DeltaType::Delta;
        s = rest;
    } else if let Some(rest) = s.strip_prefix('-') {
        sign = Sign::Minus;
        delta_type = DeltaType::Delta;
        s = rest;
    }

    let digits_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        bail!("value expression must start with a number");
    }

    let value: u16 = s[..digits_len].parse()?;
    for c in s[digits_len..].chars() {
        match c {
            '+' => {
                sign = Sign::Plus;
                delta_type = DeltaType::Delta;
            }
            '-' => {
                sign = Sign::Minus;
                delta_type = DeltaType::Delta;
            }
            '%' => value_type = ValueType::Relative,
            _ => bail!("unsupported suffix `{}` in value expression", c),
        }
    }

    Ok(ValueSpec {
        value,
        value_type,
        delta_type,
        sign,
    })
}

fn round_div(numer: i64, denom: i64) -> i64 {
    if denom == 0 {
        return 0;
    }

    if numer >= 0 {
        (numer + denom / 2) / denom
    } else {
        (numer - denom / 2) / denom
    }
}

fn percent_to_value(percent: i64, maximum: u16) -> i64 {
    round_div(percent * i64::from(maximum), 100)
}

fn value_to_percent(value: u16, maximum: u16) -> i64 {
    if maximum == 0 {
        0
    } else {
        round_div(i64::from(value) * 100, i64::from(maximum))
    }
}

fn calc_value(current: u16, maximum: u16, spec: ValueSpec) -> u16 {
    let mut new_value = i64::from(current);

    if spec.delta_type == DeltaType::Direct {
        new_value = match spec.value_type {
            ValueType::Absolute => i64::from(spec.value),
            ValueType::Relative => percent_to_value(i64::from(spec.value), maximum),
        };
    } else {
        let mut delta = i64::from(spec.value);
        if spec.sign == Sign::Minus {
            delta *= -1;
        }

        if spec.value_type == ValueType::Relative {
            delta = percent_to_value(value_to_percent(current, maximum) + delta, maximum)
                - i64::from(current);
            if spec.value != 0 && delta == 0 {
                delta = if spec.sign == Sign::Plus { 1 } else { -1 };
            }
        }

        new_value += delta;
    }

    new_value.clamp(0, i64::from(maximum)) as u16
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// output name such as DP-1
    output_name: String,
    /// feature code in decimal or 0xFF or FFh format
    #[clap(value_parser = parse_feature_code)]
    feature_code: u8,
    /// value expression: 500, 50%, 50-, +10, 50%-, +10%
    feature_value: Option<String>,
}

// /sys/class/drm/card*-{name}/i2c-*
fn get_i2c_dev(output: &str) -> Result<String> {
    let mut output_dir = None;
    for entry in read_dir("/sys/class/drm").unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.starts_with("card") && name.ends_with(output) {
            let before_name = name.len() - output.len() - 1;
            if &name[before_name..before_name + 1] == "-" {
                output_dir = Some(path);
                break;
            }
        }
    }
    let mut dev = None;
    let output_dir = output_dir.ok_or_else(|| eyre!("output name not found in /sys/class/drm"))?;
    for entry in read_dir(output_dir).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        let name = file_name.to_str().unwrap();
        if name.starts_with("i2c-") {
            dev = Some(name.to_owned());
            break;
        } else if name == "ddc" {
            let link = entry.path().read_link().unwrap();
            dev = Some(link.file_name().unwrap().to_string_lossy().into_owned())
        }
    }

    dev.ok_or_else(|| eyre!("i2c dev not found"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let i2c_name = if cli.output_name.starts_with("i2c-") {
        cli.output_name
    } else {
        get_i2c_dev(&cli.output_name)?
    };
    let dev = format!("/dev/{}", i2c_name);
    let mut ddc = ddc_i2c::from_i2c_device(dev).unwrap();
    if let Some(v) = cli.feature_value {
        let spec = parse_value(&v)?;
        let value = ddc.get_vcp_feature(cli.feature_code)?;
        let target = calc_value(value.value(), value.maximum(), spec);
        ddc.set_vcp_feature(cli.feature_code, target)?;
        println!("{}", target);
    } else {
        let value = ddc.get_vcp_feature(cli.feature_code)?;
        println!("{} {}", value.value(), value.maximum());
    }

    Ok(())
}
