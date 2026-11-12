//! A library file a build script supplies: what its name and its
//! format say of the toolchain and the machine it is for.

use std::fs::File;
use std::path::Path;

use object::read::archive::ArchiveFile;
use object::read::coff::CoffHeader as _;
use object::read::elf::FileHeader;
use object::read::macho::{FatArch, MachHeader, MachOFatFile32, MachOFatFile64};
use object::{
    Architecture, Endianness, FileKind, LittleEndian, ReadCache, ReadRef, elf, macho, pe,
};

use crate::result::{Invalid, NOT_LIB};

/// Refuse the library file at `real`, which a script named as `path`,
/// when it is not one a link of this build takes.  An MSVC-style link
/// takes a `.lib`: a file of another name there is a GNU toolchain's
/// library, which shares neither its C runtime nor its C++ ABI, or a
/// DLL, which is linked through its import library.  And no link takes
/// a file built for another machine than `machine`, the
/// [`cabin_core::TargetPlatform::arch`] the build's compilers build for.
/// A file whose format names no machine passes, as an Apple `.tbd` or a
/// linker script does.
pub(crate) fn check(path: &str, real: &Path, msvc: bool, machine: &str) -> Result<(), Invalid> {
    let lib = |extension: &std::ffi::OsStr| extension.eq_ignore_ascii_case("lib");
    if msvc && !real.extension().is_some_and(lib) {
        return Err(Invalid::Path {
            path: path.to_owned(),
            problem: NOT_LIB,
        });
    }
    // A machine the table does not name is not compared, on either side.
    let name = |machine| MACHINES.iter().find(|(built, _)| *built == machine);
    let built: Vec<_> = machines(real).into_iter().map(name).collect();
    match built.first() {
        Some(Some((_, first)))
            if MACHINES.iter().any(|(_, name)| *name == machine)
                && built
                    .iter()
                    .all(|built| built.is_some_and(|(_, built)| *built != machine)) =>
        {
            Err(Invalid::Machine {
                path: path.to_owned(),
                built: first,
                machine: machine.to_owned(),
            })
        }
        _ => Ok(()),
    }
}

/// The machines the file at `path` is built for: an object's or a shared
/// library's, each of a universal binary's, and for an archive that of
/// its first member that names one.  None for any other file, a thin
/// archive, whose members are other files, among them.
fn machines(path: &Path) -> Vec<Architecture> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    // Reads what is asked of it, so a large library is not read whole.
    let data = &ReadCache::new(file);
    match FileKind::parse(data) {
        Ok(FileKind::Archive) => ArchiveFile::parse(data)
            .ok()
            .filter(|archive| !archive.is_thin())
            .and_then(|archive| {
                archive.members().filter_map(Result::ok).find_map(|member| {
                    let (offset, size) = member.file_range();
                    machine(data.range(offset, size))
                })
            })
            .into_iter()
            .collect(),
        Ok(FileKind::MachOFat32) => MachOFatFile32::parse(data)
            .map(|fat| fat.arches().iter().map(FatArch::architecture).collect())
            .unwrap_or_default(),
        Ok(FileKind::MachOFat64) => MachOFatFile64::parse(data)
            .map(|fat| fat.arches().iter().map(FatArch::architecture).collect())
            .unwrap_or_default(),
        Ok(_) => machine(data).into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// Whether a program linked with the library file at `path` loads a
/// file when it runs, which the link did not copy into the program: a
/// shared library, an Apple text stub of one (told by the name
/// `.tbd`), or a DLL's import library in MSVC's format.  Not so a file
/// of another format, a MinGW import library and a linker script among
/// them, or one that cannot be read.
#[must_use]
pub fn loads_at_run_time(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let data = &ReadCache::new(file);
    let slice = |(offset, size)| shared(data.range(offset, size));
    match FileKind::parse(data) {
        Ok(FileKind::Archive) => ArchiveFile::parse(data).is_ok_and(|archive| {
            let mut members = archive.members().filter_map(Result::ok);
            !archive.is_thin() && members.any(|member| slice(member.file_range()))
        }),
        Ok(FileKind::MachOFat32) => MachOFatFile32::parse(data)
            .is_ok_and(|fat| fat.arches().iter().any(|arch| slice(arch.file_range()))),
        Ok(FileKind::MachOFat64) => MachOFatFile64::parse(data)
            .is_ok_and(|fat| fat.arches().iter().any(|arch| slice(arch.file_range()))),
        Ok(_) => shared(data),
        Err(_) => path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tbd")),
    }
}

/// Whether the file's header says it is a shared library, or an import
/// library's entry for one function of a DLL.
fn shared<'data>(data: impl ReadRef<'data>) -> bool {
    let header = || {
        Some(match FileKind::parse(data).ok()? {
            FileKind::Elf32 => {
                let header = elf::FileHeader32::<Endianness>::parse(data).ok()?;
                header.e_type(header.endian().ok()?) == elf::ET_DYN
            }
            FileKind::Elf64 => {
                let header = elf::FileHeader64::<Endianness>::parse(data).ok()?;
                header.e_type(header.endian().ok()?) == elf::ET_DYN
            }
            FileKind::MachO32 => {
                let header = macho::MachHeader32::<Endianness>::parse(data, 0).ok()?;
                dylib(header.filetype(header.endian().ok()?))
            }
            FileKind::MachO64 => {
                let header = macho::MachHeader64::<Endianness>::parse(data, 0).ok()?;
                dylib(header.filetype(header.endian().ok()?))
            }
            FileKind::Pe32 | FileKind::Pe64 => {
                let at = u64::from(pe::ImageDosHeader::parse(data).ok()?.nt_headers_offset()) + 4;
                let header = data.read_at::<pe::ImageFileHeader>(at).ok()?;
                header.characteristics.get(LittleEndian) & pe::IMAGE_FILE_DLL != 0
            }
            kind => kind == FileKind::CoffImport,
        })
    };
    header().unwrap_or(false)
}

fn dylib(filetype: u32) -> bool {
    matches!(filetype, macho::MH_DYLIB | macho::MH_DYLIB_STUB)
}

/// The machine of an object, a shared library, or an import library's
/// entry for one function, from the file's header alone.  A script may
/// have written the file, and reading the tables its header points at
/// can take many times the file's length in memory.
fn machine<'data>(data: impl ReadRef<'data>) -> Option<Architecture> {
    let mut at = 0;
    Some(match FileKind::parse(data).ok()? {
        FileKind::Elf32 => elf(elf::FileHeader32::<Endianness>::parse(data).ok()?)?,
        FileKind::Elf64 => elf(elf::FileHeader64::<Endianness>::parse(data).ok()?)?,
        FileKind::MachO32 => macho(macho::MachHeader32::<Endianness>::parse(data, 0).ok()?)?,
        FileKind::MachO64 => macho(macho::MachHeader64::<Endianness>::parse(data, 0).ok()?)?,
        FileKind::Coff => coff(pe::ImageFileHeader::parse(data, &mut at).ok()?.machine()),
        FileKind::CoffBig => {
            let header = pe::AnonObjectHeaderBigobj::parse(data, &mut at).ok()?;
            coff(header.machine())
        }
        FileKind::CoffImport => {
            let header = pe::ImportObjectHeader::parse(data, &mut at).ok()?;
            coff(header.machine.get(LittleEndian))
        }
        FileKind::Pe32 | FileKind::Pe64 => {
            // The file header follows the four bytes of the PE signature.
            let at = u64::from(pe::ImageDosHeader::parse(data).ok()?.nt_headers_offset()) + 4;
            coff(data.read_at::<pe::ImageFileHeader>(at).ok()?.machine())
        }
        _ => return None,
    })
}

fn elf(header: &impl FileHeader<Endian = Endianness>) -> Option<Architecture> {
    let machine = header.e_machine(header.endian().ok()?);
    Some(match (machine, header.is_class_64()) {
        (elf::EM_AARCH64, true) => Architecture::Aarch64,
        (elf::EM_ARM, _) => Architecture::Arm,
        (elf::EM_386, _) => Architecture::I386,
        (elf::EM_RISCV, true) => Architecture::Riscv64,
        (elf::EM_X86_64, true) => Architecture::X86_64,
        _ => Architecture::Unknown,
    })
}

fn macho(header: &impl MachHeader<Endian = Endianness>) -> Option<Architecture> {
    Some(match header.cputype(header.endian().ok()?) {
        macho::CPU_TYPE_ARM64 => Architecture::Aarch64,
        macho::CPU_TYPE_ARM => Architecture::Arm,
        macho::CPU_TYPE_X86 => Architecture::I386,
        macho::CPU_TYPE_X86_64 => Architecture::X86_64,
        _ => Architecture::Unknown,
    })
}

fn coff(machine: u16) -> Architecture {
    match machine {
        pe::IMAGE_FILE_MACHINE_ARM64 | pe::IMAGE_FILE_MACHINE_ARM64EC => Architecture::Aarch64,
        pe::IMAGE_FILE_MACHINE_ARMNT => Architecture::Arm,
        pe::IMAGE_FILE_MACHINE_I386 => Architecture::I386,
        pe::IMAGE_FILE_MACHINE_RISCV64 => Architecture::Riscv64,
        pe::IMAGE_FILE_MACHINE_AMD64 => Architecture::X86_64,
        _ => Architecture::Unknown,
    }
}

/// The machines a library file and a build both name, the build as
/// [`cabin_core::TargetPlatform::arch`] spells them.
const MACHINES: [(Architecture, &str); 5] = [
    (Architecture::Aarch64, "aarch64"),
    (Architecture::Arm, "arm"),
    (Architecture::I386, "x86"),
    (Architecture::Riscv64, "riscv64"),
    (Architecture::X86_64, "x86_64"),
];
