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

/// <https://tc39.es/ecma402/#sec-use-of-iana-time-zone-database>
///
/// This spec text wants us to ensure that all zones fully contained
/// in a region must canonicalize to an entry that is under zone.tab for
/// that region.
///
/// These timezones are mentioned in the packrat entries, HOWEVER the packrat
/// entries map to tzdb's canonical timezones, which doesn't include the fact that
/// we treat all zone.tab entries as canonical. There's no easy way to recover this
/// information. Instead, since there are only three of them, we assert that we have the
/// same three, and hardcode overrides.
///
/// The hardcoded values are taken from <https://github.com/unicode-org/cldr/blob/main/common/bcp47/timezone.xml>
const PACKRAT_OVERRIDES: &[(&str, &str)] = &[
    ("Atlantic/Jan_Mayen", "Arctic/Longyearbyen"),
    ("America/Coral_Harbour", "America/Atikokan"),
    ("Africa/Timbuktu", "Africa/Bamako"),
];

impl TzdbDataSource {
    /// Try to create a tzdb source from a tzdata directory.
    pub fn try_from_zoneinfo_directory(tzdata_path: &Path) -> Result<Self, TzdbDataSourceError> {
        let version_file = tzdata_path.join("version");
        let version = fs::read_to_string(version_file)?.trim().to_owned();
        let data = ZoneInfoData::from_zoneinfo_directory(tzdata_path)?;
        Ok(Self { version, data })
    }

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
    pub fn build(tzdata_path: &Path) -> Result<Self, IanaDataError> {
        let mut provider = TzdbDataSource::try_from_zoneinfo_directory(tzdata_path)
            .map_err(IanaDataError::Provider)?;

        // This data includes things like Truk/Chuuk which Temporal requires in its tests
        // It also includes the packrat data
        let backzone = ZoneInfoData::from_filepath(tzdata_path.join("backzone")).unwrap();
        provider.data.extend(backzone);

        let packrat_overrides: BTreeMap<_, _> = PACKRAT_OVERRIDES.iter().copied().collect();

        for pack in provider.data.pack_rat {
            assert!(
                packrat_overrides.contains_key(&*pack.0),
                "Found missing packrat entry {}",
                pack.0
            );
        }

        let mut all_identifiers = BTreeSet::default();

        // Add canonical identifiers.
        all_identifiers.extend(provider.data.zones.keys().map(|s| s.as_str()));

        // Add link / non-canonical identifiers
        all_identifiers.extend(provider.data.links.keys().map(|s| s.as_str()));

        let mut all_links: BTreeMap<&str, &str> = provider
            .data
            .links
            .iter()
            .map(|x| (x.0.as_str(), x.1.as_str()))
            .collect();

        // https://tc39.es/ecma402/#sec-use-of-iana-time-zone-database
        // > Any Link name that is present in the “TZ” column of file zone.tab
        // > must be a primary time zone identifier.
        //
        // So we ignore links entries that link from these timezones
        // which results in those timezones considered as primary.
        for tz in provider.data.zone_tab {
            all_links.remove(&*tz.tz);
        }

        // See comment on PACKRAT_OVERRIDES
        all_links.extend(packrat_overrides);

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

        let available_id_index = ZeroAsciiIgnoreCaseTrie::from_iter(
            all_identifiers
                .iter()
                .enumerate()
                .map(|(idx, id)| (id.to_ascii_lowercase().as_bytes().to_vec(), idx)),
        );

        let non_canonical_identifiers = all_links.iter().map(|(&link_from, &(mut link_to))| {
            // Sometimes links have multiple steps. This happens for Chungking => Chongqing => Shanghai
            while let Some(new_link_to) = all_links.get(link_to) {
                link_to = new_link_to;
            }
            (
                u32::try_from(available_id_index.get(link_from).unwrap()).unwrap(),
                u32::try_from(available_id_index.get(link_to).unwrap()).unwrap(),
            )
        });

        Ok(IanaIdentifierNormalizer {
            version: provider.version.into(),
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
