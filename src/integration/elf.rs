use std::path::Path;

use goblin::elf;

use crate::error::{SpmError, SpmResult};

pub fn get_dynamic_dependencies(path: &Path) -> SpmResult<Vec<String>> {
    let bytes = std::fs::read(path)?;
    let binary = elf::Elf::parse(&bytes)
        .map_err(|e| SpmError::other(format!("Failed to parse ELF: {e}")))?;

    Ok(binary.libraries.iter().map(|s| s.to_string()).collect())
}

pub fn set_rpath(path: &Path, rpath: &str) -> SpmResult<()> {
    let bytes = std::fs::read(path)?;

    let (strtab_off, target_d_val) = {
        let binary = elf::Elf::parse(&bytes)
            .map_err(|e| SpmError::other(format!("Failed to parse ELF: {e}")))?;

        let dynamic = match &binary.dynamic {
            Some(d) => d,
            None => {
                tracing::debug!("No dynamic section in {}", path.display());
                return Ok(());
            }
        };

        let rpath_dval = dynamic
            .dyns
            .iter()
            .find(|e| {
                e.d_tag == elf::dynamic::DT_RUNPATH || e.d_tag == elf::dynamic::DT_RPATH
            })
            .map(|e| e.d_val);

        (dynamic.info.strtab, rpath_dval)
    };

    let d_val = match target_d_val {
        Some(v) => v,
        None => {
            tracing::debug!("No RUNPATH/RPATH in {}", path.display());
            return Ok(());
        }
    };

    let file_off = strtab_off + d_val as usize;
    let old_len = bytes[file_off..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(0);

    if rpath.len() > old_len {
        tracing::warn!(
            "New RPATH ({}) is longer than old RPATH ({}) in {}, skipping",
            rpath.len(),
            old_len,
            path.display()
        );
        return Ok(());
    }

    let mut new_bytes = bytes;
    new_bytes[file_off..file_off + rpath.len()].copy_from_slice(rpath.as_bytes());
    for b in &mut new_bytes[file_off + rpath.len()..file_off + old_len] {
        *b = 0;
    }
    std::fs::write(path, new_bytes)?;
    Ok(())
}
