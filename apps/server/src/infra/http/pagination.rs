use std::str::FromStr;

use base64ct::{Base64UrlUnpadded, Encoding};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite};
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn, ParameterStyle};
use utoipa::openapi::schema::{
    ArrayBuilder, KnownFormat, ObjectBuilder, Schema, SchemaFormat, Type,
};
use utoipa::openapi::{RefOr, Required};
use utoipa::{PartialSchema, ToSchema};

use super::error::ApiError;
use crate::domain::bytes::ByteSize;
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::infra::crypto::hash::MIN_TRUNCATED_MAC_LEN;
use crate::infra::crypto::hkdf::{KeyRing, MacPurpose};

pub const DEFAULT_LIMIT: u16 = 50;
pub const MAX_LIMIT: u16 = 200;
pub const CURSOR_TAG_LEN: usize = MIN_TRUNCATED_MAC_LEN;
pub const MAX_CURSOR_CHARS: usize = 4096;
pub const SEARCH_MIN_CHARS: usize = 2;
pub const SEARCH_MAX_CHARS: usize = 128;
pub const MAX_WIRE_BYTES: i64 = 9_007_199_254_740_991;

pub const CURSOR_PARAM: &str = "cursor";
pub const LIMIT_PARAM: &str = "limit";
pub const SORT_PARAM: &str = "sort";
pub const SEARCH_PARAM: &str = "q";

pub fn invalid_param(name: &'static str) -> ApiError {
    ApiError::new(ErrorCode::ValidationError).with_detail("field", name)
}

const fn cursor_invalid() -> ApiError {
    ApiError::new(ErrorCode::CursorInvalid)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryParams(Vec<(String, String)>);

impl QueryParams {
    pub fn parse(raw: Option<&str>) -> Self {
        Self(
            raw.map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default(),
        )
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn single(&self, name: &'static str) -> Result<Option<&str>, ApiError> {
        let mut values = self.all(name);
        let first = values.next();
        if values.next().is_some() {
            return Err(invalid_param(name));
        }
        Ok(first)
    }

    pub fn repeated<T>(
        &self,
        name: &'static str,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Result<Vec<T>, ApiError> {
        self.all(name)
            .map(|value| parse(value).ok_or_else(|| invalid_param(name)))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(u16);

impl Limit {
    pub const DEFAULT: Self = Self(DEFAULT_LIMIT);

    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        let Some(raw) = raw else {
            return Ok(Self::DEFAULT);
        };
        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_param(LIMIT_PARAM));
        }
        match raw.parse::<u16>() {
            Ok(limit) if (1..=MAX_LIMIT).contains(&limit) => Ok(Self(limit)),
            _ => Err(invalid_param(LIMIT_PARAM)),
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    fn fetch_size(self) -> i64 {
        i64::from(self.0) + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery(String);

impl SearchQuery {
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, ApiError> {
        raw.map(|raw| {
            if (SEARCH_MIN_CHARS..=SEARCH_MAX_CHARS).contains(&raw.chars().count()) {
                Ok(Self(raw.to_owned()))
            } else {
                Err(invalid_param(SEARCH_PARAM))
            }
        })
        .transpose()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }

    const fn sql_order(self) -> &'static str {
        match self {
            Self::Asc => " ASC",
            Self::Desc => " DESC",
        }
    }

    const fn sql_after(self) -> &'static str {
        match self {
            Self::Asc => ") > (",
            Self::Desc => ") < (",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKeyKind {
    Text,
    Integer,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SortField {
    name: &'static str,
    column: &'static str,
    kind: SortKeyKind,
}

impl SortField {
    pub const fn new(name: &'static str, column: &'static str, kind: SortKeyKind) -> Self {
        Self { name, column, kind }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn column(&self) -> &'static str {
        self.column
    }

    pub const fn kind(&self) -> SortKeyKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    field: &'static SortField,
    direction: SortDirection,
    id_column: &'static str,
}

impl SortSpec {
    pub const fn field(&self) -> &'static SortField {
        self.field
    }

    pub const fn direction(&self) -> SortDirection {
        self.direction
    }

    pub fn wire(&self) -> String {
        format!("{}:{}", self.field.name, self.direction.as_str())
    }
}

#[derive(Debug)]
pub struct SortAllowlist {
    fields: &'static [SortField],
    default: SortSpec,
}

impl SortAllowlist {
    pub const fn new(
        fields: &'static [SortField],
        default_field: usize,
        default_direction: SortDirection,
    ) -> Self {
        Self {
            fields,
            default: SortSpec {
                field: &fields[default_field],
                direction: default_direction,
                id_column: "id",
            },
        }
    }

    #[must_use]
    pub const fn with_id_column(mut self, id_column: &'static str) -> Self {
        self.default.id_column = id_column;
        self
    }

    pub const fn default_spec(&self) -> SortSpec {
        self.default
    }

    pub fn parse(&self, raw: Option<&str>) -> Result<SortSpec, ApiError> {
        let Some(raw) = raw else {
            return Ok(self.default);
        };
        let (name, direction) = raw
            .split_once(':')
            .ok_or_else(|| invalid_param(SORT_PARAM))?;
        let direction = SortDirection::parse(direction).ok_or_else(|| invalid_param(SORT_PARAM))?;
        let field = self
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| invalid_param(SORT_PARAM))?;
        Ok(SortSpec {
            field,
            direction,
            id_column: self.default.id_column,
        })
    }

    pub fn values(&self) -> Vec<String> {
        self.fields
            .iter()
            .flat_map(|field| {
                [SortDirection::Asc, SortDirection::Desc]
                    .map(|direction| format!("{}:{}", field.name, direction.as_str()))
            })
            .collect()
    }

    pub fn parameter(&self) -> Parameter {
        query_parameter(
            SORT_PARAM,
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(self.values()))
                .default(Some(Value::String(self.default.wire())))
                .into(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SortValue {
    Text(String),
    Integer(i64),
}

impl SortValue {
    const fn kind(&self) -> SortKeyKind {
        match self {
            Self::Text(_) => SortKeyKind::Text,
            Self::Integer(_) => SortKeyKind::Integer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorKey {
    value: SortValue,
    id: String,
}

impl CursorKey {
    pub fn new<E>(value: SortValue, id: Id<E>) -> Self {
        Self {
            value,
            id: id.to_string(),
        }
    }

    pub const fn value(&self) -> &SortValue {
        &self.value
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorPayload {
    sort: String,
    k: (SortValue, String),
}

pub fn encode_cursor(keys: &KeyRing, sort: &SortSpec, key: &CursorKey) -> String {
    let payload = CursorPayload {
        sort: sort.wire(),
        k: (key.value.clone(), key.id.clone()),
    };
    let Ok(mut bytes) = serde_json::to_vec(&payload) else {
        unreachable!("a cursor payload of strings and integers always serializes");
    };
    let tag = keys.mac(MacPurpose::Cursor, &bytes);
    bytes.extend_from_slice(&tag[..CURSOR_TAG_LEN]);
    Base64UrlUnpadded::encode_string(&bytes)
}

pub fn decode_cursor(keys: &KeyRing, sort: &SortSpec, raw: &str) -> Result<CursorKey, ApiError> {
    if raw.len() > MAX_CURSOR_CHARS {
        return Err(cursor_invalid());
    }
    let bytes = Base64UrlUnpadded::decode_vec(raw).map_err(|_| cursor_invalid())?;
    let Some(split) = bytes
        .len()
        .checked_sub(CURSOR_TAG_LEN)
        .filter(|&split| split > 0)
    else {
        return Err(cursor_invalid());
    };
    let (payload, tag) = bytes.split_at(split);
    if !keys.verify_truncated_mac(MacPurpose::Cursor, payload, tag) {
        return Err(cursor_invalid());
    }
    let payload = match serde_json::from_slice::<Value>(payload) {
        Ok(object @ Value::Object(_)) => serde_json::from_value::<CursorPayload>(object),
        _ => return Err(cursor_invalid()),
    }
    .map_err(|_| cursor_invalid())?;
    let (value, id) = payload.k;
    let well_formed = payload.sort == sort.wire()
        && value.kind() == sort.field.kind
        && Id::<()>::from_str(&id).is_ok();
    if well_formed {
        Ok(CursorKey { value, id })
    } else {
        Err(cursor_invalid())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conjunction {
    Where,
    And,
}

impl Conjunction {
    const fn sql(self) -> &'static str {
        match self {
            Self::Where => " WHERE ((",
            Self::And => " AND ((",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalCount {
    Exact(u64),
    Uncounted,
}

impl From<TotalCount> for Option<u64> {
    fn from(total: TotalCount) -> Self {
        match total {
            TotalCount::Exact(count) => Some(count),
            TotalCount::Uncounted => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Opaque cursor for the next page; `null` on the last page.
    #[schema(required = true)]
    pub next_cursor: Option<String>,
    /// Matching items overall; `null` where counting would require a scan.
    #[schema(required = true, minimum = 0)]
    pub total_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    sort: SortSpec,
    after: Option<CursorKey>,
    limit: Limit,
}

impl PageRequest {
    pub fn from_query(
        params: &QueryParams,
        allowlist: &SortAllowlist,
        keys: &KeyRing,
    ) -> Result<Self, ApiError> {
        let sort = allowlist.parse(params.single(SORT_PARAM)?)?;
        let limit = Limit::parse(params.single(LIMIT_PARAM)?)?;
        let after = params
            .single(CURSOR_PARAM)?
            .map(|raw| decode_cursor(keys, &sort, raw))
            .transpose()?;
        Ok(Self { sort, after, limit })
    }

    pub const fn sort(&self) -> &SortSpec {
        &self.sort
    }

    pub const fn limit(&self) -> Limit {
        self.limit
    }

    pub const fn after(&self) -> Option<&CursorKey> {
        self.after.as_ref()
    }

    pub fn push_keyset(&self, query: &mut QueryBuilder<'_, Sqlite>, conjunction: Conjunction) {
        let Some(after) = &self.after else {
            return;
        };
        query
            .push(conjunction.sql())
            .push(self.sort.field.column)
            .push(", ")
            .push(self.sort.id_column)
            .push(self.sort.direction.sql_after());
        match &after.value {
            SortValue::Text(text) => query.push_bind(text.clone()),
            SortValue::Integer(integer) => query.push_bind(*integer),
        };
        query.push(", ").push_bind(after.id.clone()).push("))");
    }

    pub fn push_order_and_limit(&self, query: &mut QueryBuilder<'_, Sqlite>) {
        let order = self.sort.direction.sql_order();
        query
            .push(" ORDER BY ")
            .push(self.sort.field.column)
            .push(order)
            .push(", ")
            .push(self.sort.id_column)
            .push(order)
            .push(" LIMIT ")
            .push_bind(self.limit.fetch_size());
    }

    pub fn into_page<T>(
        self,
        mut rows: Vec<T>,
        keys: &KeyRing,
        key_of: impl Fn(&T, &'static SortField) -> CursorKey,
        total: TotalCount,
    ) -> Page<T> {
        let limit = usize::from(self.limit.get());
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last()
                .map(|row| encode_cursor(keys, &self.sort, &key_of(row, self.sort.field)))
        } else {
            None
        };
        Page {
            items: rows,
            next_cursor,
            total_count: total.into(),
        }
    }
}

fn query_parameter(name: &'static str, schema: RefOr<Schema>) -> Parameter {
    ParameterBuilder::new()
        .name(name)
        .parameter_in(ParameterIn::Query)
        .required(Required::False)
        .schema(Some(schema))
        .build()
}

pub fn cursor_parameter() -> Parameter {
    query_parameter(
        CURSOR_PARAM,
        ObjectBuilder::new()
            .schema_type(Type::String)
            .max_length(Some(MAX_CURSOR_CHARS))
            .into(),
    )
}

pub fn limit_parameter() -> Parameter {
    query_parameter(
        LIMIT_PARAM,
        ObjectBuilder::new()
            .schema_type(Type::Integer)
            .minimum(Some(1))
            .maximum(Some(MAX_LIMIT))
            .default(Some(Value::from(DEFAULT_LIMIT)))
            .into(),
    )
}

pub fn search_parameter() -> Parameter {
    query_parameter(
        SEARCH_PARAM,
        ObjectBuilder::new()
            .schema_type(Type::String)
            .min_length(Some(SEARCH_MIN_CHARS))
            .max_length(Some(SEARCH_MAX_CHARS))
            .into(),
    )
}

pub fn repeated_enum_parameter(name: &'static str, values: &[&'static str]) -> Parameter {
    let mut parameter = query_parameter(
        name,
        ArrayBuilder::new()
            .items(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some(values.iter().copied())),
            )
            .into(),
    );
    parameter.style = Some(ParameterStyle::Form);
    parameter.explode = Some(true);
    parameter
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WireBytes(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceedsWireRange;

impl WireBytes {
    pub const MAX: Self = Self(MAX_WIRE_BYTES);

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn clamped(size: ByteSize) -> (Self, bool) {
        match Self::try_from(size) {
            Ok(bytes) => (bytes, true),
            Err(ExceedsWireRange) => {
                tracing::warn!(
                    event = "byte_count_clamped",
                    "byte aggregate exceeds the JSON safe-integer range"
                );
                (Self::MAX, false)
            }
        }
    }

    pub fn limit(limit: Option<ByteSize>) -> Result<Option<Self>, ExceedsWireRange> {
        limit.map(Self::try_from).transpose()
    }
}

impl TryFrom<ByteSize> for WireBytes {
    type Error = ExceedsWireRange;

    fn try_from(size: ByteSize) -> Result<Self, Self::Error> {
        if size.to_i64() <= MAX_WIRE_BYTES {
            Ok(Self(size.to_i64()))
        } else {
            Err(ExceedsWireRange)
        }
    }
}

impl PartialSchema for WireBytes {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::Integer)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Int64)))
            .minimum(Some(0))
            .maximum(Some(MAX_WIRE_BYTES))
            .into()
    }
}

impl ToSchema for WireBytes {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ByteCount")
    }
}
