//! Bound each Thrift response before generated bindings allocate its contents.
use super::{sasl::FrameReader, t_c_l_i_service::*};
use std::{io::Read, mem::size_of};
use thrift::{ProtocolError, ProtocolErrorKind, protocol::*};

pub struct ResponseProtocol<R: Read> {
    inner: TBinaryInputProtocol<FrameReader<R>>,
    limit: usize,
    allocation_remaining: usize,
    active: bool,
}

fn size_error(message: &str) -> thrift::Error {
    ProtocolError::new(ProtocolErrorKind::SizeLimit, message).into()
}

impl<R: Read> ResponseProtocol<R> {
    pub fn new(mut reader: FrameReader<R>, limit: usize) -> thrift::Result<Self> {
        reader.begin_response(limit);
        let config = thrift::TConfiguration::builder()
            .max_message_size(Some(limit))
            .max_frame_size(Some(limit))
            .max_string_size(Some(limit))
            .max_container_size(Some(100_000))
            .build()?;
        Ok(Self {
            inner: TBinaryInputProtocol::with_config(reader, true, config),
            limit,
            allocation_remaining: limit,
            active: false,
        })
    }

    fn allocate(&mut self, bytes: usize) -> thrift::Result<()> {
        self.allocation_remaining = self
            .allocation_remaining
            .checked_sub(bytes)
            .ok_or_else(|| size_error("Server response exceeds allocation limit"))?;
        Ok(())
    }

    fn container(&mut self, count: i32, element_bytes: usize) -> thrift::Result<()> {
        let bytes = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(element_bytes))
            .ok_or_else(|| size_error("Invalid server container size"))?;
        self.allocate(bytes)
    }
}

// Generated bindings reserve the schema's element type without checking the
// peer's element tag. Charge the largest possible element even for scalar tags,
// so a dishonest tag cannot undercount a Vec of structs. Payloads are separate.
fn element_bytes() -> usize {
    [
        size_of::<TTypeEntry>(),
        size_of::<TColumnDesc>(),
        size_of::<TColumnValue>(),
        size_of::<TColumn>(),
        size_of::<TRow>(),
        size_of::<Vec<String>>(),
    ]
    .into_iter()
    .max()
    .unwrap()
}

macro_rules! delegate_read {
    ($($method:ident -> $result:ty),* $(,)?) => {
        $(fn $method(&mut self) -> thrift::Result<$result> { self.inner.$method() })*
    };
}

impl<R: Read> TInputProtocol for ResponseProtocol<R> {
    fn read_message_begin(&mut self) -> thrift::Result<TMessageIdentifier> {
        if self.active {
            return Err(size_error(
                "Previous server response failed; reconnect before reading again",
            ));
        }
        self.active = true;
        self.inner.transport.begin_response(self.limit);
        self.allocation_remaining = self.limit;
        // Parse the strict binary header through our own string reader so even
        // the method name is checked before its allocation.
        let version = self.read_i32()? as u32;
        if version & 0xffff_0000 != 0x8001_0000 {
            return Err(ProtocolError::new(
                ProtocolErrorKind::BadVersion,
                "Invalid binary protocol version",
            )
            .into());
        }
        let message_type = TMessageType::try_from((version & 0xff) as u8)?;
        let name = self.read_string()?;
        let sequence = self.read_i32()?;
        Ok(TMessageIdentifier::new(name, message_type, sequence))
    }

    fn read_message_end(&mut self) -> thrift::Result<()> {
        self.inner.read_message_end()?;
        self.active = false;
        Ok(())
    }

    fn read_bytes(&mut self) -> thrift::Result<Vec<u8>> {
        let length = self.read_i32()?;
        let length = usize::try_from(length).map_err(|_| {
            ProtocolError::new(ProtocolErrorKind::NegativeSize, "Negative string size")
        })?;
        if length > self.inner.transport.response_remaining() {
            return Err(size_error(
                "Server string exceeds remaining response byte limit",
            ));
        }
        self.allocate(length)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| size_error("Could not allocate server string"))?;
        bytes.resize(length, 0);
        self.inner.transport.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn read_string(&mut self) -> thrift::Result<String> {
        String::from_utf8(self.read_bytes()?).map_err(From::from)
    }

    fn read_list_begin(&mut self) -> thrift::Result<TListIdentifier> {
        let list = self.inner.read_list_begin()?;
        self.container(list.size, element_bytes())?;
        Ok(list)
    }

    fn read_set_begin(&mut self) -> thrift::Result<TSetIdentifier> {
        let set = self.inner.read_set_begin()?;
        self.container(set.size, element_bytes() + 64)?;
        Ok(set)
    }

    fn read_map_begin(&mut self) -> thrift::Result<TMapIdentifier> {
        let map = self.inner.read_map_begin()?;
        let bytes = 2 * element_bytes() + 64;
        self.container(map.size, bytes)?;
        Ok(map)
    }

    delegate_read! {
        read_struct_begin -> Option<TStructIdentifier>, read_struct_end -> (),
        read_field_begin -> TFieldIdentifier, read_field_end -> (),
        read_bool -> bool, read_i8 -> i8, read_i16 -> i16, read_i32 -> i32,
        read_i64 -> i64, read_double -> f64, read_uuid -> uuid::Uuid,
        read_list_end -> (), read_set_end -> (), read_map_end -> (), read_byte -> u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn detail(error: thrift::Error) -> String {
        match error {
            thrift::Error::Protocol(error) => error.message,
            thrift::Error::Transport(error) => error.message,
            other => panic!("Unexpected error: {other:?}"),
        }
    }

    fn message(body: impl FnOnce(&mut TBinaryOutputProtocol<&mut Vec<u8>>)) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut output = TBinaryOutputProtocol::new(&mut bytes, true);
        output
            .write_message_begin(&TMessageIdentifier::new("reply", TMessageType::Reply, 1))
            .unwrap();
        body(&mut output);
        output.write_message_end().unwrap();
        bytes
    }

    fn protocol(
        bytes: &[u8],
        frame_size: usize,
        limit: usize,
    ) -> ResponseProtocol<Cursor<Vec<u8>>> {
        let mut framed = Vec::new();
        for chunk in bytes.chunks(frame_size) {
            framed.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
            framed.extend_from_slice(chunk);
        }
        ResponseProtocol::new(FrameReader::new(Cursor::new(framed)), limit).unwrap()
    }

    #[test]
    fn aggregate_limit_applies_across_frames_and_failed_responses_cannot_reset_it() {
        let bytes = message(|output| {
            output.write_string(&"a".repeat(80)).unwrap();
            output.write_string(&"b".repeat(80)).unwrap();
        });
        let mut input = protocol(&bytes, 7, 160);
        input.read_message_begin().unwrap();
        assert_eq!(input.read_string().unwrap(), "a".repeat(80));
        assert!(detail(input.read_string().unwrap_err()).contains("remaining response byte limit"));
        assert!(
            detail(input.read_message_begin().unwrap_err())
                .contains("Previous server response failed")
        );
    }

    #[test]
    fn budgets_reset_between_responses_even_inside_one_frame() {
        let bytes = message(|output| output.write_string(&"x".repeat(40)).unwrap());
        let combined = [bytes.clone(), bytes].concat();
        for frame_size in [3, combined.len()] {
            let mut input = protocol(&combined, frame_size, 80);
            for _ in 0..2 {
                let header = input.read_message_begin().unwrap();
                assert_eq!(header.name, "reply");
                assert_eq!(input.read_string().unwrap(), "x".repeat(40));
                input.read_message_end().unwrap();
            }
        }
    }

    #[test]
    fn declared_string_and_container_sizes_are_checked_before_payload_allocation() {
        let bytes = message(|output| output.write_i32(1000).unwrap());
        let mut input = protocol(&bytes, 8, 128);
        input.read_message_begin().unwrap();
        assert!(detail(input.read_bytes().unwrap_err()).contains("remaining response byte limit"));

        let bytes = message(|output| {
            output
                .write_list_begin(&TListIdentifier::new(TType::I08, 10))
                .unwrap()
        });
        let mut input = protocol(&bytes, 8, 128);
        input.read_message_begin().unwrap();
        assert!(detail(input.read_list_begin().unwrap_err()).contains("allocation limit"));
    }

    #[test]
    fn malformed_headers_and_negative_sizes_are_rejected() {
        let mut input = protocol(&[0, 0, 0, 0], 4, 128);
        assert!(detail(input.read_message_begin().unwrap_err()).contains("version"));
        let bytes = message(|output| output.write_i32(-1).unwrap());
        let mut input = protocol(&bytes, 8, 128);
        input.read_message_begin().unwrap();
        assert!(detail(input.read_bytes().unwrap_err()).contains("Negative"));
    }

    #[test]
    fn scalar_reads_cannot_cross_response_limit() {
        let bytes = message(|output| {
            for value in 0..30 {
                output.write_i64(value).unwrap();
            }
        });
        let mut input = protocol(&bytes, 4, 128);
        input.read_message_begin().unwrap();
        for value in 0..13 {
            assert_eq!(input.read_i64().unwrap(), value);
        }
        assert!(detail(input.read_i64().unwrap_err()).contains("response exceeds byte limit"));
    }
}
