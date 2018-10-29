// Copyright (c) 2018 King's College London
// created by the Software Development Team <http://soft-dev.org/>
//
// The Universal Permissive License (UPL), Version 1.0
//
// Subject to the condition set forth below, permission is hereby granted to any person obtaining a
// copy of this software, associated documentation and/or data (collectively the "Software"), free
// of charge and under any and all copyright rights in the Software, and any and all patent rights
// owned or freely licensable by each licensor hereunder covering either (i) the unmodified
// Software as contributed to or provided by such licensor, or (ii) the Larger Works (as defined
// below), to deal in both
//
// (a) the Software, and
// (b) any piece of software and/or hardware listed in the lrgrwrks.txt file
// if one is included with the Software (each a "Larger Work" to which the Software is contributed
// by such licensors),
//
// without restriction, including without limitation the rights to copy, create derivative works
// of, display, perform, and distribute the Software and make, use, sell, offer for sale, import,
// export, have made, and have sold the Software and the Larger Work(s), and to sublicense the
// foregoing rights on either these or other terms.
//
// This license is subject to the following condition: The above copyright notice and either this
// complete permission notice or at a minimum a reference to the UPL must be included in all copies
// or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
// BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

/// Parse the MIR control flow graph encoded into binaries compiled with `ykrustc`.

use std::path::Path;
use std::io::{Cursor, Seek, SeekFrom};
use byteorder::{NativeEndian, ReadBytesExt};
use std::collections::{HashMap, HashSet};
use elf;
use super::MirLoc;
use super::errors::TraceMappingError;

const MIR_CFG_SECTION_NAME: &str = ".yk_mir_cfg";
const MIR_CFG_SECTION_VERSION: u16 = 0;

// Edge kinds.
const GOTO: u8 = 0;
const SWITCHINT: u8 = 1;
const RESUME: u8 = 2;
const ABORT: u8 = 3;
const RETURN: u8 = 4;
const UNREACHABLE: u8 = 5;
const DROP_NO_UNWIND: u8 = 6;
const DROP_WITH_UNWIND: u8 = 7;
const DROP_AND_REPLACE_NO_UNWIND: u8 = 8;
const DROP_AND_REPLACE_WITH_UNWIND: u8 = 9;
const CALL_NO_CLEANUP: u8 = 10;
const CALL_WITH_CLEANUP: u8 = 11;
const CALL_UNKNOWN_NO_CLEANUP: u8 = 12;
const CALL_UNKNOWN_WITH_CLEANUP: u8 = 13;
const ASSERT_NO_CLEANUP: u8 = 14;
const ASSERT_WITH_CLEANUP: u8 = 15;
const YIELD_NO_DROP: u8 = 16;
const YIELD_WITH_DROP: u8 = 17;
const GENERATOR_DROP: u8 = 18;
const FALSE_EDGES: u8 = 19;
const FALSE_UNWIND: u8 = 20;
const NO_MIR: u8 = 254;
const SENTINAL: u8 = 255;

/// Represents a block terminator.
/// This is pretty much a mirror of the type of the same name in rustc.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TerminatorKind {
    Goto{target_bb: u32},
    SwitchInt{targets: Vec<u32>},
    Resume,
    Abort,
    Return,
    Unreachable,
    Drop{target_bb: u32, unwind_bb: Option<u32>},
    DropAndReplace{target_bb: u32, unwind_bb: Option<u32>},
    Call{target: Option<(u64, u32)>, cleanup_bb: Option<u32>},
    Assert{target_bb: u32, cleanup_bb: Option<u32>},
    Yield{resume_bb: u32, drop_bb: Option<u32>},
    GeneratorDrop,
    FalseEdges{real_target_bb: u32},
    FalseUnwind{real_target_bb: u32},
}

/// Slurps the serialised MIR CFG from an ELF section and presents it like a hash map.
pub (crate) struct MirCfg {
    /// Describes the MIR CFG via a mapping from block to a collection of successors. There may be
    /// >1 successor because any given DefId may be translated many times for different type
    /// specialisation.
    _map: HashMap<MirLoc, TerminatorKind>,
    // DefIds for which there was no MIR availabel at compile time.
    _no_mir_def_ids: HashSet<(u64, u32)>,
}

/// Describes MIR locations that we know about.
#[derive(Debug)]
pub enum _KnownMirLoc<'a> {
    /// We know this location and have MIR for it.
    HasMir(&'a TerminatorKind),
    /// We know this location, but it doesn't have any MIR.
    NoMir,
}

impl MirCfg {
    pub fn new(path: &Path) -> Result<Self, TraceMappingError> {
        let elf_file = elf::File::open_path(path).unwrap();
        let mut curs = cursor_from_elf(&elf_file, MIR_CFG_SECTION_NAME, 0)?;

        let mut map = HashMap::new();
        let mut no_mir_def_ids = HashSet::new();

        let vers = curs.read_u16::<NativeEndian>().unwrap();
        if vers != MIR_CFG_SECTION_VERSION {
            panic!("MIR CFG section version mismatch");
        }

        loop {
            let typ = curs.read_u8().unwrap();

            if typ == NO_MIR {
                let crate_hash = curs.read_u64::<NativeEndian>().unwrap();
                let idx = curs.read_u32::<NativeEndian>().unwrap();
                no_mir_def_ids.insert((crate_hash, idx));
                continue;
            }

            let (blk, term) = match typ {
                GOTO => {
                    //eprintln!("GOTO");
                    let (blk, target_bb) = Self::parse_loc_and_target_bb(&mut curs);
                    let term = TerminatorKind::Goto{target_bb};
                    (blk, term)
                },
                SWITCHINT => {
                    //eprintln!("SWITCHINT");
                    let blk = Self::parse_loc(&mut curs);
                    let num_targets = curs.read_uint::<NativeEndian>(8).unwrap(); // XXX
                    let mut targets = Vec::new();
                    for _ in 0..num_targets {
                        targets.push(curs.read_u32::<NativeEndian>().unwrap());
                    }
                    let term = TerminatorKind::SwitchInt{targets};
                    (blk, term)
                },
                RESUME | ABORT | RETURN | UNREACHABLE => {
                    //eprintln!("RESUME, ABORT, RETURN, UNREACHABLE");
                    let blk = Self::parse_loc(&mut curs);
                    let term = match typ {
                        RESUME => TerminatorKind::Resume,
                        ABORT => TerminatorKind::Abort,
                        RETURN => TerminatorKind::Return,
                        UNREACHABLE=> TerminatorKind::Unreachable,
                        _ => panic!("un-handled case"),
                    };
                    (blk, term)
                },
                DROP_NO_UNWIND | DROP_WITH_UNWIND | DROP_AND_REPLACE_NO_UNWIND | DROP_AND_REPLACE_WITH_UNWIND => {
                    //eprintln!("DROP_NO_UNWIND | DROP_WITH_UNWIND | DROP_AND_REPLACE_NO_UNWIND | DROP_AND_REPLACE_WITH_UNWIND");
                    let (blk, target_bb)  = Self::parse_loc_and_target_bb(&mut curs);
                    let unwind_bb = if typ == DROP_NO_UNWIND || typ == DROP_AND_REPLACE_NO_UNWIND {
                        None
                    } else {
                        Some(curs.read_u32::<NativeEndian>().unwrap())
                    };
                    let term = if typ == DROP_NO_UNWIND || typ == DROP_WITH_UNWIND {
                        TerminatorKind::Drop{target_bb, unwind_bb}
                    } else {
                        TerminatorKind::DropAndReplace{target_bb, unwind_bb}
                    };
                    (blk, term)
                },
                CALL_NO_CLEANUP | CALL_WITH_CLEANUP | CALL_UNKNOWN_NO_CLEANUP | CALL_UNKNOWN_WITH_CLEANUP => {
                    let blk = Self::parse_loc(&mut curs);

                    let target = if typ != CALL_UNKNOWN_NO_CLEANUP && typ != CALL_UNKNOWN_WITH_CLEANUP {
                        // Known target. Encode it's DefId.
                        let t_krate_hash = curs.read_u64::<NativeEndian>().unwrap();
                        let t_idx = curs.read_u32::<NativeEndian>().unwrap();
                        Some((t_krate_hash, t_idx))
                    } else {
                        // Unknown target.
                        None
                    };

                    let cleanup_bb = if typ == CALL_WITH_CLEANUP || typ == CALL_UNKNOWN_WITH_CLEANUP {
                        // Has cleanup.
                        Some(curs.read_u32::<NativeEndian>().unwrap())
                    } else {
                        // No cleanup.
                        None
                    };

                    (blk, TerminatorKind::Call{target, cleanup_bb})
                },
                ASSERT_NO_CLEANUP | ASSERT_WITH_CLEANUP => {
                    //eprintln!("ASSERT_NO_CLEANUP | ASSERT_WITH_CLEANUP");
                    let (blk, target_bb) = Self::parse_loc_and_target_bb(&mut curs);
                    let cleanup_bb = if typ == ASSERT_WITH_CLEANUP {
                        Some(curs.read_u32::<NativeEndian>().unwrap())
                    } else {
                        None
                    };
                    let term = TerminatorKind::Assert{target_bb, cleanup_bb};
                    (blk, term)
                },
                YIELD_NO_DROP | YIELD_WITH_DROP => {
                    //eprintln!("YIELD_NO_DROP | YIELD_WITH_DROP");
                    let (blk, resume_bb) = Self::parse_loc_and_target_bb(&mut curs);
                    let drop_bb = if typ == ASSERT_WITH_CLEANUP {
                        Some(curs.read_u32::<NativeEndian>().unwrap())
                    } else {
                        None
                    };
                    let term = TerminatorKind::Yield{resume_bb, drop_bb};
                    (blk, term)
                },
                GENERATOR_DROP => {
                    //eprintln!("GENERATOR_DROP");
                    (Self::parse_loc(&mut curs), TerminatorKind::GeneratorDrop)
                },
                FALSE_EDGES | FALSE_UNWIND => {
                    //eprintln!("FALSE_EDGES | FALSE_UNWIND");
                    let (blk, real_target_bb) = Self::parse_loc_and_target_bb(&mut curs);
                    let term = if typ == FALSE_EDGES {
                        TerminatorKind::FalseEdges{real_target_bb}
                    } else {
                        TerminatorKind::FalseUnwind{real_target_bb}
                    };
                    (blk, term)
                },
                SENTINAL => {
                    eprintln!("SENTINAL");
                    //eprintln!("end @ 0x{:x}", curs.position());
                    //panic!("stop!"); // XXX
                    break;
                }
                _ => panic!("unknown type"),
            };


            let mut do_insert = true;
            {
                let opt_old = map.get(&blk);
                // Rustc may allocate >1 crate number for the same crate. When duplicate crate
                // meta-data is requested, the compiler will point us at already loaded meta-data
                // where the DefIds within reference a different crate number. Due to this, it's
                // possible to see DefIds (and thus their blocks) that we've already encountered.
                // So it's OK if the map already has an entry for any given block, but we do check
                // that it has the same terminator as what we saw before.
                if let Some(old) = opt_old {
                    if *old != term {
                        panic!("already an entry for this block with a different terminator");
                    }
                    do_insert = false;
                }
            }
            if do_insert {
                map.insert(blk, term);
            }
        }

        Ok(Self{_map: map, _no_mir_def_ids: no_mir_def_ids})
    }

    /// Parse a `MirLoc` and a basic block number from the binary CFG section.
    fn parse_loc_and_target_bb(curs: &mut Cursor<&Vec<u8>>) -> (MirLoc, u32) {
        let block = Self::parse_loc(curs);
        let to_bb = curs.read_u32::<NativeEndian>().unwrap();
        (block, to_bb)
    }

    /// Parse a `MirLoc` from the binary CFG section.
    fn parse_loc(curs: &mut Cursor<&Vec<u8>>) -> MirLoc {
        let crate_hash = curs.read_u64::<NativeEndian>().unwrap();
        let def_idx = curs.read_u32::<NativeEndian>().unwrap();
        let bb = curs.read_u32::<NativeEndian>().unwrap();
        MirLoc::new(crate_hash, def_idx, bb)
    }

    /// Attempt to lookup MIR CFG information about a `MirLoc`.
    pub fn _get(&self, key: &MirLoc) -> Result<_KnownMirLoc, ()> {
        match self._map.get(key) {
            Some(t) => Ok(_KnownMirLoc::HasMir(t)),
            None => {
                if self._no_mir_def_ids.contains(&(key.crate_hash, key.idx)) {
                    Ok(_KnownMirLoc::NoMir)
                } else {
                    Err(())
                }
            },
        }
    }
}

fn cursor_from_elf<'a>(elf_file: &'a elf::File, section_name: &'a str, start_pos: u64)
                               -> Result<Cursor<&'a Vec<u8>>, TraceMappingError> {
    let sec_res = elf_file.get_section(section_name);

    if let Some(sec) = sec_res {
        let mut cursor = Cursor::new(&sec.data);
        cursor.seek(SeekFrom::Start(start_pos))?;
        Ok(cursor)
    } else {
        Err(TraceMappingError::NoCfg)
    }
}

#[cfg(test)]
mod tests {
    use super::MirCfg;
    use std::env;

    /// If we build the tests with ykrustc, then this binary will have a CFG section which we can
    /// parse as a test.
    #[test]
    fn test_parser() {
        let cfg = MirCfg::new(&env::current_exe().unwrap()).unwrap();
        assert!(cfg._map.len() > 0);
    }
}
