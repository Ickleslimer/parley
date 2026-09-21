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

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
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
    let mut digest = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
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
    let mut working_zero = state[0];
    let mut working_one = state[1];
    let mut working_two = state[2];
    let mut working_three = state[3];
    let mut working_four = state[4];
    let mut working_five = state[5];
    let mut working_six = state[6];
    let mut working_seven = state[7];
    for index in 0..64 {
        let sigma_one = working_four.rotate_right(6)
            ^ working_four.rotate_right(11)
            ^ working_four.rotate_right(25);
        let choose = (working_four & working_five) ^ (!working_four & working_six);
        let first = working_seven
            .wrapping_add(sigma_one)
            .wrapping_add(choose)
            .wrapping_add(ROUND_CONSTANTS[index])
            .wrapping_add(words[index]);
        let sigma_zero = working_zero.rotate_right(2)
            ^ working_zero.rotate_right(13)
            ^ working_zero.rotate_right(22);
        let majority = (working_zero & working_one)
            ^ (working_zero & working_two)
            ^ (working_one & working_two);
        let second = sigma_zero.wrapping_add(majority);
        working_seven = working_six;
        working_six = working_five;
        working_five = working_four;
        working_four = working_three.wrapping_add(first);
        working_three = working_two;
        working_two = working_one;
        working_one = working_zero;
        working_zero = first.wrapping_add(second);
    }
    state[0] = state[0].wrapping_add(working_zero);
    state[1] = state[1].wrapping_add(working_one);
    state[2] = state[2].wrapping_add(working_two);
    state[3] = state[3].wrapping_add(working_three);
    state[4] = state[4].wrapping_add(working_four);
    state[5] = state[5].wrapping_add(working_five);
    state[6] = state[6].wrapping_add(working_six);
    state[7] = state[7].wrapping_add(working_seven);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_vectors() {
        assert_eq!(
            hex_encode(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
