extern crate combine;

use std::marker::PhantomData;
use std::str;

use combine::byte::*;
use combine::combinator::*;
use combine::range::*;
use combine::primitives::{Consumed, ParseResult, Parser, RangeStream};


fn is_option_char(c: u8) -> bool {
    match c {
        b'a'...b'z' | b'A'...b'Z' | b'0'...b'9' | b'-' | b'_' => true,
        _ => false,
    }
}

struct DiffxParser<I>(PhantomData<I>);
impl<'a, I> DiffxParser<I>
    where I: RangeStream<Item = u8, Range = &'a [u8]>
{
    // Parse an option key or value.
    fn option_str(input: I) -> ParseResult<&'a str, I> {
        // The call to str::from_utf8_unchecked is safe due to is_option_char
        // only accepting a limited subset of ASCII.
        take_while1(is_option_char)
            .map(|s| unsafe { str::from_utf8_unchecked(s) })
            .parse_stream(input)
    }

    // Parse an option.
    //
    // Options are key-vaue pairs separated by `=`.
    fn option(input: I) -> ParseResult<(&'a str, &'a str), I> {
        (parser(DiffxParser::<I>::option_str),
         byte('=' as u8).with(parser(DiffxParser::<I>::option_str)))
                .parse_stream(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_option() {
        assert_eq!(DiffxParser::option(&b"foo=bar"[..]),
                   Ok((("foo", "bar"), Consumed::Consumed(&b""[..]))));

        assert_eq!(DiffxParser::option(&b"encoding=utf-8"[..]),
                   Ok((("encoding", "utf-8"), Consumed::Consumed(&b""[..]))));
    }
}
