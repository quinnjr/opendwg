//! The bit reader. The input is a program: the first byte is the number of
//! reads, the next bytes pick each read, the rest is the bit stream. Every
//! read is bounds-checked, so any program must end with `Ok` or an error.
#![no_main]

use libfuzzer_sys::fuzz_target;
use opendwg::Version;
use opendwg::bits::BitReader;

const VERSIONS: [Version; 6] = [
    Version::R2000,
    Version::R2004,
    Version::R2007,
    Version::R2010,
    Version::R2013,
    Version::R2018,
];

fuzz_target!(|data: &[u8]| {
    let Some((&count, rest)) = data.split_first() else {
        return;
    };
    let count = usize::from(count % 64);
    let Some((ops, stream)) = rest.split_at_checked(count) else {
        return;
    };
    let mut r = BitReader::new(stream);
    for &op in ops {
        let version = VERSIONS[usize::from(op >> 5) % VERSIONS.len()];
        let _ = match op % 32 {
            0 => r.read_b().map(drop),
            1 => r.read_bb().map(drop),
            2 => r.read_3b().map(drop),
            3 => r.read_bs().map(drop),
            4 => r.read_bl().map(drop),
            5 => r.read_bll().map(drop),
            6 => r.read_bd().map(drop),
            7 => r.read_rc().map(drop),
            8 => r.read_rs().map(drop),
            9 => r.read_rl().map(drop),
            10 => r.read_rd().map(drop),
            11 => r.read_bytes(usize::from(op)).map(drop),
            12 => r.read_mc().map(drop),
            13 => r.read_umc().map(drop),
            14 => r.read_ms().map(drop),
            15 => r.read_dd(1.0).map(drop),
            16 => r.read_bt().map(drop),
            17 => r.read_be().map(drop),
            18 => r.read_2rd().map(drop),
            19 => r.read_2dd([1.0, 2.0]).map(drop),
            20 => r.read_3bd().map(drop),
            21 => r.read_3rd().map(drop),
            22 => r.read_ot(version).map(drop),
            23 => r.read_handle_ref().map(drop),
            24 => r.read_tv_raw().map(drop),
            25 => r.read_tu_checked().map(drop),
            26 => r.read_cmc(version).map(drop),
            27 => r.read_cmc_layer(version).map(drop),
            28 => r.read_enc(version).map(drop),
            29 => r.seek_bit(u64::from(op) * 3),
            30 => {
                r.align_byte();
                Ok(())
            }
            _ => Ok(()),
        };
    }
});
