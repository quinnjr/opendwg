//! Objects of types the library does not decode, kept as bits.

use super::remap::{Mapper, clears_offset, rewrite_ref};
use super::stream::ObjReader;
use crate::bits::BitReader;
use crate::error::Result;
use crate::handle::{Handle, HandleRef};
use crate::version::Version;

/// The type-specific part of an object of an unsupported type, kept as bits,
/// with its handle references parsed so they can be renumbered. The common
/// data is decoded and held beside it (see [`super::Object`]), so ownership
/// and handles stay editable.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RawObject {
    /// The type code: a fixed code, or a class number (500 and up).
    pub type_code: u16,
    /// DXF class name, for class-numbered types.
    pub class_name: Option<String>,
    /// The version of the file the bits were read from.
    pub source_version: Version,
    /// Type-specific data stream bits after the common data, zero-padded.
    pub data: Vec<u8>,
    /// How many bits of `data` are real.
    pub data_bits: u64,
    /// R2007+: the string stream bits not read by the common data.
    pub strings: Option<(Vec<u8>, u64)>,
    /// Type-specific handle references, in stream order.
    pub handles: Vec<HandleRef>,
    /// Handle stream bits after the last whole reference: the end padding
    /// (AutoCAD pads with one bits, and counts them in the handle stream
    /// size), MSB-first and zero-padded to a byte.
    pub tail_handles_raw: Vec<u8>,
    /// How many bits of `tail_handles_raw` are real.
    pub tail_handle_bits: u64,
}

impl RawObject {
    /// Keep the rest of each stream of `r`, which the caller has read up to
    /// the end of the common data.
    pub fn decode(
        r: &mut ObjReader,
        type_code: u16,
        class_name: Option<String>,
    ) -> Result<RawObject> {
        let (data, data_bits) = take_rest(&mut r.data)?;
        let strings = r.strings.as_mut().map(take_rest).transpose()?;
        // Both layouts end the handle stream at the object's last byte and pad
        // only to that byte boundary, so fewer than 8 padding bits remain after
        // the last reference and no phantom reference is parsed. A writer that
        // adds a whole zero byte of padding would yield a {code 0, value 0}
        // reference; that re-encodes to the same 0x00 byte, so nothing is lost.
        let mut handles = Vec::new();
        while r.handles.remaining_bits() >= 8 {
            let at = r.handles.bit_pos();
            match r.handles.read_handle_ref() {
                Ok(h) => handles.push(h),
                Err(_) => {
                    r.handles.seek_bit(at)?;
                    break;
                }
            }
        }
        let (tail_handles_raw, tail_handle_bits) = take_rest(&mut r.handles)?;
        Ok(RawObject {
            type_code,
            class_name,
            source_version: r.version,
            data,
            data_bits,
            strings,
            handles,
            tail_handles_raw,
            tail_handle_bits,
        })
    }

    /// Rewrite the kept handle references for a handle renumbering: `own`
    /// is the object's handle before it, and `f` maps every handle (the
    /// object's own included; it is not called for the null handle, which
    /// stays null). An absolute reference (codes 0-5) keeps its code. An
    /// offset from the object's handle (codes 6, 8, 0xA, 0xC) is kept while
    /// neither the object nor the target moves; otherwise it becomes the
    /// absolute target with code 4, the soft pointer every offset stands
    /// for (ODS 2.13). An offset that names no handle from `own` (it runs
    /// below 0 or past the largest handle) is kept while the object stays;
    /// once it moves the offset would name an unrelated object, so it
    /// becomes a null soft pointer, and the references so cleared are
    /// returned. References with other codes are kept. Handles inside the
    /// kept data bits cannot be found and are kept as stored.
    pub fn rewrite_handles(&mut self, own: Handle, f: impl Fn(Handle) -> Handle) -> Vec<HandleRef> {
        let mut f = f;
        let mut m = Mapper::new(&mut f);
        let new_own = m.map(own);
        let mut cleared = Vec::new();
        for r in &mut self.handles {
            let old = *r;
            *r = rewrite_ref(old, own, new_own, &mut m);
            if clears_offset(old, own, new_own) {
                cleared.push(old);
            }
        }
        cleared
    }

    /// Whether the kept bits can be written as `target`. Bits are only copied
    /// between versions that share the R2007+ stream layout; R2000/R2004 bits
    /// are only valid in their own version.
    pub fn is_convertible_to(&self, target: Version) -> bool {
        target == self.source_version || self.source_version >= Version::R2007
    }
}

/// An object whose common data could not be read (a damaged file), kept as
/// stored with why, so reading never drops it silently. It is held as a
/// non-graphical object whatever its type, owns and refers to nothing the
/// drawing knows of, and cannot be written:
/// [`crate::Drawing::unconvertible_objects`] lists it; remove it to save the
/// drawing.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnreadableObject {
    /// The type code, when it could be read.
    pub type_code: Option<u16>,
    /// The DXF class name, for a class-numbered type the drawing has a class
    /// of.
    pub class_name: Option<String>,
    /// The version of the file it was read from.
    pub source_version: Version,
    /// The object as stored, from its size field to its CRC.
    pub bytes: Vec<u8>,
    /// Why its common data could not be read.
    pub error: String,
}

/// The remaining bits of `r`, MSB-first and zero-padded, and their count.
pub(crate) fn take_rest(r: &mut BitReader) -> Result<(Vec<u8>, u64)> {
    let bits = r.remaining_bits();
    Ok((take_bits(r, bits)?, bits))
}

/// The next `bits` bits of `r`, MSB-first and zero-padded (checked against
/// the bits left before anything is allocated).
pub(crate) fn take_bits(r: &mut BitReader, bits: u64) -> Result<Vec<u8>> {
    if bits > r.remaining_bits() {
        return Err(crate::error::Error::Truncated { context: "bits" });
    }
    let mut out = r.read_bytes((bits / 8) as usize)?;
    let rest = (bits % 8) as u32;
    if rest > 0 {
        let mut last = 0u8;
        for i in 0..rest {
            if r.read_b()? {
                last |= 0x80 >> i;
            }
        }
        out.push(last);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::handle::Handle;
    use crate::object::common::{ObjectCommon, decode_object_common, encode_object_common};
    use crate::object::stream::ObjWriter;
    use crate::object::stream::tests::assemble;

    fn common() -> ObjectCommon {
        ObjectCommon {
            handle: Handle(0x80),
            owner: Handle(0xC),
            owner_relative: false,
            reactors: vec![Handle(0xC)],
            xdict: None,
            xdata: vec![],
            has_ds_binary_data: false,
        }
    }

    #[test]
    fn raw_object_keeps_rest() {
        let mut w = ObjWriter::new(Handle(0x80));
        encode_object_common(&mut w, &common()).unwrap();
        w.data.write_bs(300);
        w.data.write_b(true);
        w.text("custom").unwrap();
        w.href(HandleRef {
            code: 5,
            value: 0x99,
        })
        .unwrap();
        w.href(HandleRef {
            code: 2,
            value: 0x1234,
        })
        .unwrap();
        let bytes = w.finish(0x1F4).unwrap();
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        assert_eq!(decode_object_common(&mut r).unwrap(), common());
        let raw = RawObject::decode(&mut r, pre.type_code, Some("ACME_THING".into())).unwrap();
        assert_eq!(raw.type_code, 0x1F4);
        let mut expect = BitWriter::new();
        expect.write_bs(300);
        expect.write_b(true);
        assert_eq!(raw.data_bits, expect.bit_len());
        assert_eq!(raw.data, expect.into_bytes());
        let mut s = BitWriter::new();
        s.write_tu("custom").unwrap();
        assert_eq!(raw.strings, Some((s.clone().into_bytes(), s.bit_len())));
        assert_eq!(
            raw.handles,
            [
                HandleRef {
                    code: 5,
                    value: 0x99
                },
                HandleRef {
                    code: 2,
                    value: 0x1234
                }
            ]
        );
        assert!(raw.tail_handles_raw.len() <= 1);
        assert!(raw.is_convertible_to(Version::R2018));
    }

    #[test]
    fn r2000_raw_is_not_convertible() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x80,
        })
        .unwrap();
        d.write_bs(0);
        d.write_bl(0);
        d.write_bl(77);
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef {
            code: 4,
            value: 0xC,
        })
        .unwrap();
        hs.write_handle_ref(HandleRef { code: 3, value: 0 })
            .unwrap();
        hs.write_handle_ref(HandleRef {
            code: 5,
            value: 0x10,
        })
        .unwrap();
        let bytes = assemble(Version::R2000, 0x1F4, &d, None, &hs);
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2000, 30).unwrap();
        decode_object_common(&mut r).unwrap();
        let raw = RawObject::decode(&mut r, pre.type_code, None).unwrap();
        assert_eq!(raw.strings, None);
        assert_eq!(
            raw.handles,
            [HandleRef {
                code: 5,
                value: 0x10
            }]
        );
        assert!(raw.is_convertible_to(Version::R2000));
        assert!(!raw.is_convertible_to(Version::R2018));
    }

    #[test]
    fn whole_zero_pad_byte_reencodes_identically() {
        let mut d = BitWriter::new();
        d.write_handle_ref(HandleRef {
            code: 0,
            value: 0x80,
        })
        .unwrap();
        d.write_bs(0);
        d.write_bl(0);
        d.write_b(true);
        d.write_b(false);
        let mut hs = BitWriter::new();
        hs.write_handle_ref(HandleRef {
            code: 4,
            value: 0xC,
        })
        .unwrap();
        hs.write_handle_ref(HandleRef {
            code: 5,
            value: 0x10,
        })
        .unwrap();
        hs.write_rc(0); // a non-conforming extra padding byte
        let bytes = assemble(Version::R2018, 0x1F4, &d, None, &hs);
        let (mut r, pre) = ObjReader::split(&bytes, Version::R2018, 30).unwrap();
        decode_object_common(&mut r).unwrap();
        let raw = RawObject::decode(&mut r, pre.type_code, None).unwrap();
        let null = HandleRef { code: 0, value: 0 };
        assert_eq!(
            raw.handles,
            [
                HandleRef {
                    code: 5,
                    value: 0x10
                },
                null
            ]
        );
        let mut w = BitWriter::new();
        w.write_handle_ref(null).unwrap();
        assert_eq!(w.into_bytes(), [0]);
    }

    /// References kept as stored follow a handle rewrite: absolute ones
    /// keep their code, offsets from the object's own handle become
    /// absolute soft pointers (ODS 2.13) once that handle moves, and resolve
    /// to the mapped targets; null references are kept, and an offset that
    /// names no handle is cleared (and returned) once the object moves.
    #[test]
    fn raw_rewrite_handles() {
        let own = Handle(0x80);
        let refs = [
            HandleRef {
                code: 0xA,
                value: 0x10,
            }, // own + 0x10 = 0x90
            HandleRef { code: 6, value: 0 }, // 0x81
            HandleRef { code: 8, value: 0 }, // 0x7F
            HandleRef {
                code: 0xC,
                value: 0x20,
            }, // 0x60
            HandleRef {
                code: 3,
                value: 0x99,
            },
            HandleRef { code: 4, value: 0 },
            HandleRef {
                code: 0xC,
                value: 0x100,
            }, // below 0: unresolvable
            HandleRef { code: 7, value: 5 }, // unknown code
        ];
        let raw = |handles: &[HandleRef]| RawObject {
            type_code: 0x1F4,
            class_name: Some("ACME_THING".into()),
            source_version: Version::R2018,
            data: vec![0xA5],
            data_bits: 7,
            strings: None,
            handles: handles.to_vec(),
            tail_handles_raw: vec![0xE0],
            tail_handle_bits: 3,
        };
        let shift = |h: Handle| Handle(h.0 + 0x1000);
        let mut r = raw(&refs);
        assert_eq!(r.rewrite_handles(own, shift), [refs[6]]);
        let new_own = shift(own);
        let resolved: Vec<Option<Handle>> = r.handles.iter().map(|x| x.resolve(new_own)).collect();
        assert_eq!(
            resolved[..5],
            [0x1090, 0x1081, 0x107F, 0x1060, 0x1099].map(|v| Some(Handle(v)))
        );
        assert_eq!(
            r.handles[..5].iter().map(|x| x.code).collect::<Vec<_>>(),
            [4, 4, 4, 4, 3]
        );
        // Null kept; the offset that names no handle would name an
        // unrelated object from the new handle: cleared; unknown code kept.
        assert_eq!(r.handles[5], refs[5]);
        assert_eq!(r.handles[6], HandleRef { code: 4, value: 0 });
        assert_eq!(r.handles[7], refs[7]);
        assert_eq!(
            (
                &r.data,
                r.data_bits,
                &r.tail_handles_raw,
                r.tail_handle_bits
            ),
            (&vec![0xA5], 7, &vec![0xE0], 3),
            "the bits are kept"
        );
        // The rewritten object encodes the mapped references.
        let mut w = ObjWriter::recording(new_own);
        crate::object::write_raw(&mut w, &r).unwrap();
        assert_eq!(w.recorded_refs(), r.handles);

        // The object stays put and so do the targets of its offsets: they
        // are kept as they are; an absolute reference still follows.
        let mut r = raw(&refs);
        let cleared = r.rewrite_handles(own, |h| if h == Handle(0x99) { Handle(0x55) } else { h });
        assert!(cleared.is_empty(), "the object stays: nothing is cleared");
        let mut expect = refs;
        expect[4].value = 0x55;
        assert_eq!(r.handles, expect);
        // Only the target of one offset moves: that one becomes absolute.
        let mut r = raw(&refs);
        r.rewrite_handles(own, |h| if h == Handle(0x90) { Handle(0x500) } else { h });
        let mut expect = refs;
        expect[0] = HandleRef {
            code: 4,
            value: 0x500,
        };
        assert_eq!(r.handles, expect);
    }

    #[test]
    fn take_rest_partial_byte() {
        let data = [0b1011_0110, 0b1110_0000];
        let mut r = BitReader::with_bit_limit(&data, 11);
        r.seek_bit(2).unwrap();
        assert_eq!(
            take_rest(&mut r).unwrap(),
            (vec![0b1101_1011, 0b1000_0000], 9)
        );
        assert_eq!(r.remaining_bits(), 0);
    }
}
