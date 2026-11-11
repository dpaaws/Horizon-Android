//! Minecraft's `FriendlyByteBuf` as Simple Voice Chat writes it: big-endian
//! numbers, VarInt lengths, UUIDs as two longs and UTF-8 strings.

pub type Result<T> = std::result::Result<T, &'static str>;

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err("packet too short");
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().expect("N bytes"))
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.array()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }
    pub fn uuid(&mut self) -> Result<u128> {
        Ok(u128::from_be_bytes(self.array()?))
    }
    pub fn varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            value |= u32::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err("VarInt too big")
    }
    /// `readByteArray(max)`.
    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.varint()?;
        if len < 0 || len as usize > max {
            return Err("byte array too long");
        }
        self.bytes(len as usize)
    }
    /// `readUtf(max)`: at most `max` UTF-16 units, `max * 3` bytes.
    pub fn utf(&mut self, max: usize) -> Result<String> {
        let len = self.varint()?;
        if len < 0 || len as usize > max * 3 {
            return Err("string too long");
        }
        let s = std::str::from_utf8(self.bytes(len as usize)?).map_err(|_| "bad UTF-8")?;
        if s.encode_utf16().count() > max {
            return Err("string too long");
        }
        Ok(s.to_owned())
    }
}

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn uuid(&mut self, v: u128) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn varint(&mut self, v: i32) -> &mut Self {
        let mut v = v as u32;
        loop {
            if v & !0x7F == 0 {
                self.0.push(v as u8);
                return self;
            }
            self.0.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }
    pub fn byte_array(&mut self, v: &[u8]) -> &mut Self {
        self.varint(v.len() as i32).raw(v)
    }
    /// `writeUtf`; the caller keeps the text within the reader's limit.
    pub fn utf(&mut self, v: &str) -> &mut Self {
        self.byte_array(v.as_bytes())
    }
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip() {
        for v in [0, 1, 127, 128, 255, 2047, 25565, 2097151, i32::MAX, -1, i32::MIN] {
            let bytes = Writer::new().varint(v).finish();
            assert_eq!(Reader::new(&bytes).varint().unwrap(), v);
        }
        assert_eq!(Writer::new().varint(300).finish(), [0xAC, 0x02]);
    }

    #[test]
    fn uuids_are_two_big_endian_longs() {
        let id = 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210u128;
        let bytes = Writer::new().uuid(id).finish();
        assert_eq!(&bytes[..8], &0x0123_4567_89AB_CDEFu64.to_be_bytes());
        assert_eq!(Reader::new(&bytes).uuid().unwrap(), id);
    }

    #[test]
    fn strings_keep_their_limits() {
        let bytes = Writer::new().utf("Steve").finish();
        assert_eq!(bytes, b"\x05Steve");
        assert_eq!(Reader::new(&bytes).utf(16).unwrap(), "Steve");
        assert!(Reader::new(&bytes).utf(4).is_err());
    }
}
