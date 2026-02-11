use super::*;
use alloc::string::String;
use alloc::vec::Vec;
use std::{
    borrow::ToOwned,
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
};
use zoneinfo_rs::{ZoneInfoData, ZoneInfoError};

#[derive(Debug)]
pub enum TzdbDataSourceError {
    Io(io::Error),
    ZoneInfo(ZoneInfoError),
}

impl From<io::Error> for TzdbDataSourceError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ZoneInfoError> for TzdbDataSourceError {
    fn from(value: ZoneInfoError) -> Self {
        Self::ZoneInfo(value)
    }
}

pub struct TzdbDataSource {
    pub version: String,
    pub data: ZoneInfoData,
}

impl TzdbDataSource {
    /// Try to create a tzdb source from a tzdata rearguard.zi
    ///
    /// To generate a rearguard.zi, download tzdata from IANA. Run `make rearguard.zi`
    pub fn try_from_rearguard_zoneinfo_dir(
        tzdata_path: &Path,
    ) -> Result<Self, TzdbDataSourceError> {
        let version_file = tzdata_path.join("version");
        let version = fs::read_to_string(version_file)?.trim().to_owned();
        let rearguard_zoneinfo = tzdata_path.join("rearguard.zi");
        let data = ZoneInfoData::from_filepath(rearguard_zoneinfo)?;
        Ok(Self { version, data })
    }
}

// ==== Begin DataProvider impl ====

#[derive(Debug)]
pub enum IanaDataError {
    Io(io::Error),
    Provider(TzdbDataSourceError),
    Build(zerotrie::ZeroTrieBuildError),
}

#[allow(clippy::expect_used, clippy::unwrap_used, reason = "Datagen only")]
impl IanaIdentifierNormalizer<'_> {
    pub fn build(_tzdata_path: &Path) -> Result<Self, IanaDataError> {
        let mut all_identifiers = icu_time::zone::iana::IanaParserExtended::new()
            .iter_all()
            .flat_map(|n| [n.canonical, n.normalized])
            .collect::<BTreeSet<_>>();

        let mut all_links: BTreeMap<&str, &str> = icu_time::zone::iana::IanaParserExtended::new()
            .iter_all()
            .map(|n| (n.normalized, n.canonical))
            .filter(|(a, b)| a != b)
            .collect();

        // ECMAScript implementations must support an available named time zone with the identifier "UTC", which must be
        // the primary time zone identifier for the UTC time zone.
        all_links.remove("UTC");
        all_links.retain(|_k, v| {
            if matches!(*v, "Etc/UTC" | "Etc/GMT") {
                *v = "UTC";
            }
            true
        });
        all_links.insert("Etc/UTC", "UTC");
        all_links.insert("Etc/GMT", "UTC");

        // Don't use these zones from CLDR
        all_identifiers.remove("Canada/East-Saskatchewan");
        all_links.remove("Canada/East-Saskatchewan");
        all_identifiers.remove("US/Pacific-New");
        all_links.remove("US/Pacific-New");

        // Insert this zone that CLDR doesn't have
        all_identifiers.insert("Asia/Hanoi");

        let available_id_index = ZeroAsciiIgnoreCaseTrie::from_iter(
            all_identifiers
                .iter()
                .enumerate()
                .map(|(idx, id)| (id.to_ascii_lowercase().as_bytes().to_vec(), idx)),
        );

        let non_canonical_identifiers = all_links.iter().map(|(&link_from, &link_to)| {
            (
                u32::try_from(available_id_index.get(link_from).unwrap()).unwrap(),
                u32::try_from(available_id_index.get(link_to).unwrap()).unwrap(),
            )
        });

        Ok(IanaIdentifierNormalizer {
            version: "2025c".into(),
            non_canonical_identifiers: non_canonical_identifiers.collect(),
            available_id_index: available_id_index.convert_store(),
            normalized_identifiers: all_identifiers
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
        })
    }
}

// ==== End DataProvider impl ====
