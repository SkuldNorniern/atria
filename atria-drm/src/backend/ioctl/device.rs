use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use libc::{EINVAL, EIO, ENOENT, EOPNOTSUPP};

use crate::backend::ioctl::uapi::{
    CAP_DUMB_BUFFER, CLIENT_CAP_ATOMIC, CardResources, GetCap, GetConnector, GetEncoder,
    IOCTL_DROP_MASTER, IOCTL_GET_CAP, IOCTL_MODE_GETCONNECTOR, IOCTL_MODE_GETENCODER,
    IOCTL_MODE_GETRESOURCES, IOCTL_SET_CLIENT_CAP, IOCTL_SET_MASTER, MODE_CONNECTED, ModeInfo,
    SetClientCap, ioctl,
};
use crate::{DrmCapabilities, DrmError, Mode, ModeSelection, select_mode};

const ENUMERATION_ATTEMPTS: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceConfig {
    pub card_index: u32,
    pub connector_id: Option<u32>,
    pub mode: ModeSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connector {
    pub id: u32,
    pub encoder_id: u32,
    pub encoder_ids: Vec<u32>,
    pub modes: Vec<Mode>,
    pub connected: bool,
    pub(crate) raw_modes: Vec<ModeInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Encoder {
    pub id: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSnapshot {
    pub connectors: Vec<Connector>,
    pub encoders: Vec<Encoder>,
    pub crtc_ids: Vec<u32>,
}

#[derive(Debug)]
pub struct DrmDevice {
    file: File,
    capabilities: DrmCapabilities,
    resources: ResourceSnapshot,
    connector_index: usize,
    mode_index: usize,
    crtc_id: u32,
    master: bool,
}

impl DrmDevice {
    pub fn open(config: DeviceConfig) -> Result<Self, DrmError> {
        let (path, path_len) = device_path(config.card_index);
        let path = Path::new(OsStr::from_bytes(&path[..path_len]));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| match error.raw_os_error() {
                Some(ENOENT) => DrmError::DeviceAbsent {
                    card_index: config.card_index,
                },
                Some(errno) => DrmError::OpenDevice { errno },
                None => DrmError::OpenDevice { errno: EIO },
            })?;

        let mut no_argument = ();
        ioctl(&file, IOCTL_SET_MASTER, &mut no_argument)
            .map_err(|errno| DrmError::SetMaster { errno })?;

        let selected = Self::select_resources(&file, config);
        match selected {
            Ok((capabilities, resources, connector_index, mode_index, crtc_id)) => Ok(Self {
                file,
                capabilities,
                resources,
                connector_index,
                mode_index,
                crtc_id,
                master: true,
            }),
            Err(error) => {
                let mut no_argument = ();
                let _ = ioctl(&file, IOCTL_DROP_MASTER, &mut no_argument);
                Err(error)
            }
        }
    }

    fn select_resources(
        file: &File,
        config: DeviceConfig,
    ) -> Result<(DrmCapabilities, ResourceSnapshot, usize, usize, u32), DrmError> {
        let dumb_buffers = get_capability(file, CAP_DUMB_BUFFER)? != 0;
        let atomic_commit = enable_atomic(file)?;
        let resources = enumerate_resources(file)?;
        let connector_index = choose_connector(&resources.connectors, config.connector_id)?;
        let mode_index = select_mode(&resources.connectors[connector_index].modes, config.mode)?;
        let crtc_id = choose_crtc(&resources, connector_index)?;
        Ok((
            DrmCapabilities::new(dumb_buffers, atomic_commit),
            resources,
            connector_index,
            mode_index,
            crtc_id,
        ))
    }

    #[must_use]
    pub const fn capabilities(&self) -> DrmCapabilities {
        self.capabilities
    }

    #[must_use]
    pub fn resources(&self) -> &ResourceSnapshot {
        &self.resources
    }

    #[must_use]
    pub fn connector(&self) -> &Connector {
        &self.resources.connectors[self.connector_index]
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.connector().modes[self.mode_index]
    }

    #[must_use]
    pub const fn crtc_id(&self) -> u32 {
        self.crtc_id
    }

    pub(crate) const fn file(&self) -> &File {
        &self.file
    }

    pub(crate) fn raw_mode(&self) -> ModeInfo {
        self.connector().raw_modes[self.mode_index]
    }
}

impl Drop for DrmDevice {
    fn drop(&mut self) {
        if self.master {
            let mut no_argument = ();
            let _ = ioctl(&self.file, IOCTL_DROP_MASTER, &mut no_argument);
            self.master = false;
        }
    }
}

fn get_capability(file: &File, capability: u64) -> Result<u64, DrmError> {
    let mut query = GetCap {
        capability,
        value: 0,
    };
    ioctl(file, IOCTL_GET_CAP, &mut query)
        .map_err(|errno| DrmError::GetCapability { capability, errno })?;
    Ok(query.value)
}

fn enable_atomic(file: &File) -> Result<bool, DrmError> {
    let mut request = SetClientCap {
        capability: CLIENT_CAP_ATOMIC,
        value: 1,
    };
    match ioctl(file, IOCTL_SET_CLIENT_CAP, &mut request) {
        Ok(()) => Ok(true),
        Err(EOPNOTSUPP | EINVAL) => Ok(false),
        Err(errno) => Err(DrmError::EnableAtomicClient { errno }),
    }
}

fn enumerate_resources(file: &File) -> Result<ResourceSnapshot, DrmError> {
    for _ in 0..ENUMERATION_ATTEMPTS {
        let mut query = CardResources::default();
        ioctl(file, IOCTL_MODE_GETRESOURCES, &mut query)
            .map_err(|errno| DrmError::EnumerateResources { errno })?;
        let mut crtc_ids = vec![0; count(query.count_crtcs)?];
        let mut connector_ids = vec![0; count(query.count_connectors)?];
        let mut encoder_ids = vec![0; count(query.count_encoders)?];
        query.crtc_id_ptr = pointer(&mut crtc_ids);
        query.connector_id_ptr = pointer(&mut connector_ids);
        query.encoder_id_ptr = pointer(&mut encoder_ids);
        ioctl(file, IOCTL_MODE_GETRESOURCES, &mut query)
            .map_err(|errno| DrmError::EnumerateResources { errno })?;
        if query.count_crtcs as usize > crtc_ids.len()
            || query.count_connectors as usize > connector_ids.len()
            || query.count_encoders as usize > encoder_ids.len()
        {
            continue;
        }
        crtc_ids.truncate(query.count_crtcs as usize);
        connector_ids.truncate(query.count_connectors as usize);
        encoder_ids.truncate(query.count_encoders as usize);
        let connectors = connector_ids
            .into_iter()
            .map(|id| query_connector(file, id))
            .collect::<Result<Vec<_>, _>>()?;
        let encoders = encoder_ids
            .into_iter()
            .map(|id| query_encoder(file, id))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ResourceSnapshot {
            connectors,
            encoders,
            crtc_ids,
        });
    }
    Err(DrmError::ResourcesChanged)
}

fn query_connector(file: &File, connector_id: u32) -> Result<Connector, DrmError> {
    for _ in 0..ENUMERATION_ATTEMPTS {
        let mut query = GetConnector {
            connector_id,
            ..GetConnector::default()
        };
        ioctl(file, IOCTL_MODE_GETCONNECTOR, &mut query).map_err(|errno| {
            DrmError::QueryConnector {
                connector_id,
                errno,
            }
        })?;
        let mut raw_modes = vec![ModeInfo::default(); count(query.count_modes)?];
        let mut encoder_ids = vec![0; count(query.count_encoders)?];
        let mut property_ids = vec![0; count(query.count_props)?];
        let mut property_values = vec![0_u64; count(query.count_props)?];
        query.modes_ptr = pointer(&mut raw_modes);
        query.encoders_ptr = pointer(&mut encoder_ids);
        query.props_ptr = pointer(&mut property_ids);
        query.prop_values_ptr = pointer(&mut property_values);
        ioctl(file, IOCTL_MODE_GETCONNECTOR, &mut query).map_err(|errno| {
            DrmError::QueryConnector {
                connector_id,
                errno,
            }
        })?;
        if query.count_modes as usize > raw_modes.len()
            || query.count_encoders as usize > encoder_ids.len()
            || query.count_props as usize > property_ids.len()
        {
            continue;
        }
        raw_modes.truncate(query.count_modes as usize);
        encoder_ids.truncate(query.count_encoders as usize);
        let modes = raw_modes
            .iter()
            .map(|mode| Mode {
                width: mode.hdisplay,
                height: mode.vdisplay,
                refresh_hz: mode.vrefresh,
                mode_type: mode.mode_type,
            })
            .collect();
        return Ok(Connector {
            id: connector_id,
            encoder_id: query.encoder_id,
            encoder_ids,
            modes,
            connected: query.connection == MODE_CONNECTED,
            raw_modes,
        });
    }
    Err(DrmError::ResourcesChanged)
}

fn query_encoder(file: &File, encoder_id: u32) -> Result<Encoder, DrmError> {
    let mut query = GetEncoder {
        encoder_id,
        ..GetEncoder::default()
    };
    ioctl(file, IOCTL_MODE_GETENCODER, &mut query)
        .map_err(|errno| DrmError::QueryEncoder { encoder_id, errno })?;
    Ok(Encoder {
        id: encoder_id,
        crtc_id: query.crtc_id,
        possible_crtcs: query.possible_crtcs,
    })
}

fn choose_connector(connectors: &[Connector], requested: Option<u32>) -> Result<usize, DrmError> {
    match requested {
        Some(id) => connectors
            .iter()
            .position(|connector| connector.id == id && connector.connected)
            .ok_or(DrmError::RequestedConnectorUnavailable(id)),
        None => connectors
            .iter()
            .position(|connector| connector.connected)
            .ok_or(DrmError::NoConnectedConnector),
    }
}

fn choose_crtc(resources: &ResourceSnapshot, connector_index: usize) -> Result<u32, DrmError> {
    let connector = &resources.connectors[connector_index];
    let current = resources
        .encoders
        .iter()
        .find(|encoder| encoder.id == connector.encoder_id);
    let encoders = current
        .into_iter()
        .chain(resources.encoders.iter().filter(|encoder| {
            connector.encoder_ids.contains(&encoder.id) && Some(encoder.id) != current.map(|e| e.id)
        }));
    for encoder in encoders {
        if encoder.crtc_id != 0 && resources.crtc_ids.contains(&encoder.crtc_id) {
            return Ok(encoder.crtc_id);
        }
        for (index, crtc_id) in resources.crtc_ids.iter().copied().enumerate() {
            let bit = 1_u32.checked_shl(index as u32).unwrap_or(0);
            if encoder.possible_crtcs & bit != 0 {
                return Ok(crtc_id);
            }
        }
    }
    if connector.encoder_ids.is_empty() {
        Err(DrmError::ConnectorHasNoEncoder(connector.id))
    } else {
        Err(DrmError::NoCompatibleCrtc {
            connector_id: connector.id,
        })
    }
}

fn count(value: u32) -> Result<usize, DrmError> {
    usize::try_from(value).map_err(|_| DrmError::ArithmeticOverflow)
}

fn pointer<T>(values: &mut [T]) -> u64 {
    values.as_mut_ptr() as usize as u64
}

fn device_path(card_index: u32) -> ([u8; 32], usize) {
    let mut path = [0_u8; 32];
    let prefix = b"/dev/dri/card";
    path[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0_u8; 10];
    let mut value = card_index;
    let mut digit_count = 0;
    loop {
        digits[digit_count] = b'0' + (value % 10) as u8;
        digit_count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in 0..digit_count {
        path[prefix.len() + index] = digits[digit_count - index - 1];
    }
    (path, prefix.len() + digit_count)
}
