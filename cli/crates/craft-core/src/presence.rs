use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Distinguishes an absent patch field from an explicit clear and a replacement value.
/// Containing fields must use `serde(default, skip_serializing_if = "InputField::is_omitted")`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputField<T> {
    #[default]
    Omitted,
    Null,
    Value(T),
}

impl<T> InputField<T> {
    pub fn is_omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }
}

impl<T: Serialize> Serialize for InputField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Omitted => Err(serde::ser::Error::custom(
                "An omitted field must be skipped by its containing object",
            )),
            Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for InputField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

impl<T: JsonSchema> JsonSchema for InputField<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("InputField_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <Option<T>>::json_schema(generator)
    }
}
