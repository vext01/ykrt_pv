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

/// This module houses the mechanisms for mapping PT traces to MIR traces.

pub mod errors;
mod mir_cfg;

use std::fmt::{self, Debug};
use std::env;
use trace_mapping::mir_cfg::MirCfg;
use self::errors::TraceMappingError;

/// Represents a location in the MIR.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct MirLoc {
    crate_hash: u64,
    idx: u32,
    bb: u32,
}

impl MirLoc {
    pub fn new(crate_hash: u64, idx: u32, bb: u32) -> Self {
        Self{crate_hash, idx, bb}
    }

    pub fn _crate_hash(&self) -> u64 {
        self.crate_hash
    }

    pub fn _def_idx(&self) -> u32 {
        self.idx
    }

    pub fn _basic_block(&self) -> u32 {
        self.bb
    }
}

impl Debug for MirLoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MirLoc(0x{:08x}, {}, {})", self.crate_hash, self.idx, self.bb)
    }
}

/// The top-level "context" for performing trace mappings.
pub struct MappingCtxt {
    /// Describes the control flow graph (CFG) of the MIR.
    _cfg: MirCfg,
}

impl MappingCtxt {
    pub fn new() -> Result<Self, TraceMappingError> {
        let exe_path = env::current_exe().unwrap();
        Ok(Self {
            _cfg: MirCfg::new(&exe_path)?,
        })
    }
}
