const ROUND_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn digest(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut offset = 0;
    while offset + 64 <= data.len() {
        let mut block = [0_u8; 64];
        block.copy_from_slice(&data[offset..offset + 64]);
        compress(&mut state, &block);
        offset += 64;
    }
    let mut tail = [0_u8; 128];
    let remaining = data.len() - offset;
    tail[..remaining].copy_from_slice(&data[offset..]);
    tail[remaining] = 0x80;
    let bit_length = (data.len() as u64).saturating_mul(8);
    let length_offset = if remaining < 56 { 56 } else { 120 };
    tail[length_offset..length_offset + 8].copy_from_slice(&bit_length.to_be_bytes());
    compress(&mut state, tail.first_chunk().expect("first SHA-256 block"));
    if length_offset == 120 {
        compress(
            &mut state,
            tail[64..].first_chunk().expect("second SHA-256 block"),
        );
    }
    let mut out = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex_encode(&digest(data))
}

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut words = [0_u32; 64];
    for (index, word) in words.iter_mut().enumerate().take(16) {
        let start = index * 4;
        *word = u32::from_be_bytes([
            block[start],
            block[start + 1],
            block[start + 2],
            block[start + 3],
        ]);
    }
    for index in 16..64 {
        let sigma_zero = words[index - 15].rotate_right(7)
            ^ words[index - 15].rotate_right(18)
            ^ (words[index - 15] >> 3);
        let sigma_one = words[index - 2].rotate_right(17)
            ^ words[index - 2].rotate_right(19)
            ^ (words[index - 2] >> 10);
        words[index] = words[index - 16]
            .wrapping_add(sigma_zero)
            .wrapping_add(words[index - 7])
            .wrapping_add(sigma_one);
    }
    let mut working = *state;
    for index in 0..64 {
        let sum_one =
            working[4].rotate_right(6) ^ working[4].rotate_right(11) ^ working[4].rotate_right(25);
        let choice = (working[4] & working[5]) ^ (!working[4] & working[6]);
        let temp_one = working[7]
            .wrapping_add(sum_one)
            .wrapping_add(choice)
            .wrapping_add(ROUND_CONSTANTS[index])
            .wrapping_add(words[index]);
        let sum_zero =
            working[0].rotate_right(2) ^ working[0].rotate_right(13) ^ working[0].rotate_right(22);
        let majority =
            (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
        let temp_zero = sum_zero.wrapping_add(majority);
        working[7] = working[6];
        working[6] = working[5];
        working[5] = working[4];
        working[4] = working[3].wrapping_add(temp_one);
        working[3] = working[2];
        working[2] = working[1];
        working[1] = working[0];
        working[0] = temp_one.wrapping_add(temp_zero);
    }
    for (slot, word) in state.iter_mut().zip(working) {
        *slot = slot.wrapping_add(word);
    }
}
