extern crate combine;
#[macro_use]
extern crate maplit;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::str;

use combine::byte::*;
use combine::combinator::*;
use combine::range::*;
use combine::primitives::{ParseResult, Parser, RangeStream};


#[derive(Debug, PartialEq, Eq)]
struct SectionHeader<'a> {
    depth: usize,
    title: &'a str,
    options: HashMap<&'a str, &'a str>,
}

fn is_option_char(c: u8) -> bool {
    match c {
        b'a'...b'z' | b'A'...b'Z' | b'0'...b'9' | b'-' | b'_' | b'.' => true,
        _ => false,
    }
}

fn is_section_header_char(c: u8) -> bool {
    match c {
        b'a'...b'z' | b'A'...b'Z' | b'-' => true,
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

    // Parse an option list.
    //
    // Option lists are a list of options separated by `,`. The result is
    // collected into a HashMap for convenience.
    fn option_list(input: I) -> ParseResult<HashMap<&'a str, &'a str>, I> {
        sep_by(parser(DiffxParser::<I>::option), byte(',' as u8))
            .map(|tuples: Vec<_>| tuples.into_iter().collect())
            .parse_stream(input)
    }

    // Parse a section header.
    fn section_header(input: I) -> ParseResult<SectionHeader<'a>, I> {
        let depth = take_while(|c| c == b'.').map(|xs: &[_]| xs.len());

        // Again, the call str::from_utf8_unchecked is safe due to
        // is_section_header_char only accepting a limited subset of ASCII.
        let title = take_while(is_section_header_char)
            .map(|s| unsafe { str::from_utf8_unchecked(s) });

        let option_list = skip_many1(byte(b' ')).with(parser(DiffxParser::<I>::option_list));

        byte(b'#')
            .with((depth, title.skip(byte(b':')), optional(option_list)))
            .skip(skip_many(byte(b' ')))
            .skip(byte(b'\n'))
            .map(|(depth, title, maybe_options)| {
                SectionHeader {
                    depth: depth,
                    title: title,
                    options: maybe_options.unwrap_or_else(HashMap::new),
                }
            })
            .parse_stream(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_option() {
        assert_eq!(parser(DiffxParser::option).parse(&b"foo=bar"[..]),
                   Ok((("foo", "bar"), &b""[..])));

        assert_eq!(parser(DiffxParser::option).parse(&b"encoding=utf-8"[..]),
                   Ok((("encoding", "utf-8"), &b""[..])));

        assert_eq!(parser(DiffxParser::option).parse(&b"version=1.0"[..]),
                   Ok((("version", "1.0"), &b""[..])));
    }

    #[test]
    fn test_option_list() {
        assert_eq!(parser(DiffxParser::option_list).parse(&b"foo=bar"[..]),
                   Ok((hashmap!{ "foo" => "bar" }, &b""[..])));

        assert_eq!(parser(DiffxParser::option_list).parse(&b"encoding=utf-8,version=1.0"[..]),
                   Ok((hashmap!{ "encoding" => "utf-8", "version" => "1.0" }, &b""[..])));
    }

    #[test]
    fn test_section_header() {
        assert_eq!(parser(DiffxParser::section_header)
                       .parse(&b"#diffx: version=1.0,encoding=utf-8\n"[..]),
                   Ok((SectionHeader {
                           depth: 0,
                           title: "diffx",
                           options: hashmap!{
                               "version" => "1.0",
                               "encoding" => "utf-8",
                           },
                       },
                       &b""[..])));

        assert_eq!(parser(DiffxParser::section_header)
                       .parse(&b"#..sub-section: content-length=128\n"[..]),
                   Ok((SectionHeader {
                           depth: 2,
                           title: "sub-section",
                           options: hashmap!{ "content-length" => "128" },
                       },
                       &b""[..])));

        assert_eq!(parser(DiffxParser::section_header).parse(&b"#.section:     \n"[..]),
                   Ok((SectionHeader {
                           depth: 1,
                           title: "section",
                           options: hashmap!{},
                       },
                       &b""[..])));

        assert_eq!(parser(DiffxParser::section_header)
                       .parse(&b"#.section:   encoding=utf-8   \n"[..]),
                   Ok((SectionHeader {
                           depth: 1,
                           title: "section",
                           options: hashmap!{ "encoding" => "utf-8" },
                       },
                       &b""[..])));
    }
}
