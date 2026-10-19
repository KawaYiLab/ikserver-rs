//! 宽松的参数反序列化：模型偶尔会把数字、布尔写成字符串（"10"、"true"），一律接受。

use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum NumOrStr {
    Num(f64),
    Str(String),
}

pub fn opt_usize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    let v: Option<NumOrStr> = Option::deserialize(d)?;
    match v {
        None => Ok(None),
        Some(NumOrStr::Num(n)) if n.is_finite() && n >= 0.0 => Ok(Some(n as usize)),
        Some(NumOrStr::Num(_)) => Err(serde::de::Error::custom("需要非负整数")),
        Some(NumOrStr::Str(s)) if s.trim().is_empty() => Ok(None),
        Some(NumOrStr::Str(s)) => s
            .trim()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("「{s}」不是整数"))),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BoolOrStr {
    Bool(bool),
    Str(String),
}

pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    let v: Option<BoolOrStr> = Option::deserialize(d)?;
    match v {
        None => Ok(None),
        Some(BoolOrStr::Bool(b)) => Ok(Some(b)),
        Some(BoolOrStr::Str(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "" => Ok(None),
            "true" | "1" | "yes" | "是" => Ok(Some(true)),
            "false" | "0" | "no" | "否" => Ok(Some(false)),
            _ => Err(serde::de::Error::custom(format!("「{s}」不是布尔值"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Args {
        #[serde(default, deserialize_with = "super::opt_usize")]
        n: Option<usize>,
        #[serde(default, deserialize_with = "super::opt_bool")]
        b: Option<bool>,
    }

    #[test]
    fn accepts_strings_and_numbers() {
        let a: Args = serde_json::from_str(r#"{"n":"12","b":"true"}"#).unwrap();
        assert_eq!((a.n, a.b), (Some(12), Some(true)));
        let a: Args = serde_json::from_str(r#"{"n":3,"b":false}"#).unwrap();
        assert_eq!((a.n, a.b), (Some(3), Some(false)));
        let a: Args = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!((a.n, a.b), (None, None));
        assert!(serde_json::from_str::<Args>(r#"{"n":"abc"}"#).is_err());
        assert!(serde_json::from_str::<Args>(r#"{"n":-1}"#).is_err());
    }
}
