extern crate combine;
#[macro_use]
extern crate maplit;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::str;

use combine::byte::*;
use combine::combinator::*;
use combine::primitives::{Consumed, Error, ParseError, ParseResult, Parser, RangeStream};
use combine::range::*;


#[derive(Debug, PartialEq, Eq)]
/// A section of a DiffX document.
pub struct Section<'a> {
    /// The encoding of this section.
    ///
    /// If this was not specified in the options, it will default to the parent
    /// encoding, if any. Otherwise, it will be [`Encoding::Binary`][binary].
    ///
    /// [binary]: enum.Encoding.html#Binary.v
    pub encoding: Encoding,

    /// The options of this section.
    pub options: HashMap<&'a str, &'a str>,

    /// The content of this section.
    ///
    /// Sections cannot be empty. They can have data (either encoded or
    /// unencoded) or one or more child sections.
    pub content: SectionContent<'a>,
}

#[derive(Debug, PartialEq, Eq)]
/// The content of a section.
pub enum SectionContent<'a> {
    /// One or more sections.
    ChildSections(HashMap<&'a str, Section<'a>>),

    /// Encoded data.
    ///
    /// This section contains a string in a particular encoding, indicated by
    /// the `encoding` of the section which contains it.
    EncodedData(&'a str),

    /// Raw binary data.
    RawData(&'a [u8]),
}

use SectionContent::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// An enumeration representing possible encoding of DiffX sections.
pub enum Encoding {
    /// Unencoded data.
    ///
    /// Sections with this encoding will be treated as a sequence of bytes.
    Binary,

    /// UTF-8 encoded data.
    ///
    /// Sections with this encoding will be treated as UTF-8 encoded strings.
    Utf8,
}

impl Encoding {
    fn from_str(s: &str) -> Option<Encoding> {
        match s {
            "utf-8" => Some(Encoding::Utf8),
            "binary" => Some(Encoding::Binary),
            _ => None,
        }
    }
}

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

    // Return a parser that can parse a section of a given depth.
    //
    // The `parent_encoding` will be used as the encoding of the section if it
    // does not specify one.
    fn section(depth: usize,
               parent_encoding: Encoding)
               -> Box<Parser<Input = I, Output = (&'a str, Section<'a>)> + 'a>
        where I: 'a
    {
        parser(move |input: I| {
                let start = input.position();
                let (header, input) = try!(parser(DiffxParser::<I>::section_header)
                    .parse_stream(input));

                if header.depth != depth {
                    return Err(Consumed::Consumed(ParseError::new(start, Error::Expected(
                               format!("section with depth {}", depth).into()))));
                }

                let encoding = match header.options.get("encoding") {
                    Some(encoding) => {
                        match Encoding::from_str(encoding) {
                            Some(encoding) => encoding,
                            None => {
                                let msg = ["encoding", encoding].join(" ");
                                let err = Error::Unexpected(msg.into());
                                return Err(Consumed::Consumed(ParseError::new(start, err)));
                            }
                        }
                    }
                    None => parent_encoding,
                };

                let (content, input) = try!(DiffxParser::<I>::section_content(&header, encoding)
                    .parse_stream(input.into_inner()));

                Ok(((header.title,
                     Section {
                         encoding: encoding,
                         options: header.options,
                         content: content,
                     }),
                    input))
            })
            .boxed()
    }

    // Return a parser that will parse the content of a section given its header.
    fn section_content<'b>(section_header: &'b SectionHeader<'a>,
                           encoding: Encoding)
                           -> Box<Parser<Input = I, Output = (SectionContent<'a>)> + 'b>
        where I: 'a
    {
        parser(move |input: I| {
                let start = input.position();
                let content_length = match section_header.options.get("content-length") {
                    Some(content_length) => {
                        match content_length.parse() {
                            Ok(content_length) => Some(content_length),
                            Err(_) => {
                                let msg = ["content-length", content_length].join(" ");
                                let err = Error::Unexpected(msg.into());
                                return Err(Consumed::Consumed(ParseError::new(start, err)));
                            }
                        }
                    }
                    None => None,
                };

                let (content, input) = try!(if let Some(content_length) = content_length {
                    take(content_length)
                        .and_then(|bs| match encoding {
                            Encoding::Binary => Ok(RawData(bs)),
                            Encoding::Utf8 => str::from_utf8(bs).map(EncodedData),
                        })
                        .skip(byte(b'\n'))
                        .parse_stream(input)
                } else {
                    many1(try(DiffxParser::<I>::section(section_header.depth + 1, encoding)))
                        .map(ChildSections)
                        .parse_stream(input)
                });

                Ok((content, input))
            })
            .boxed()
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

    #[test]
    fn test_section() {
        assert_eq!(DiffxParser::section(0, Encoding::Binary)
                       .parse(&b"#diffx: version=1.0,encoding=utf-8,content-length=0\n\n"[..]),
                   Ok((("diffx",
                        Section {
                            encoding: Encoding::Utf8,
                            options: hashmap!{
                                "version" => "1.0",
                                "encoding" => "utf-8",
                                "content-length" => "0",
                            },
                            content: EncodedData(""),
                        }),
                       &b""[..])));

        assert_eq!(DiffxParser::section(0, Encoding::Binary).parse(&b"\
#diffx: version=1.0,encoding=utf-8
#.foo: content-length=14
Hello, \xE4\xB8\x96\xE7\x95\x8C

#.bar: content-length=16,encoding=binary
Goodbye, world!

"[..]),
                   Ok((("diffx",
                        Section {
                            encoding: Encoding::Utf8,
                            options: hashmap!{
                                "version" => "1.0",
                                "encoding" => "utf-8",
                            },
                            content: ChildSections(hashmap!{
                                "foo" => Section {
                                    encoding: Encoding::Utf8,
                                    options: hashmap!{ "content-length" => "14" },
                                    content: EncodedData("Hello, 世界\n")
                                },
                                "bar" => Section {
                                    encoding: Encoding::Binary,
                                    options: hashmap!{
                                        "content-length" => "16",
                                        "encoding" => "binary",
                                    },
                                    content: RawData(&b"Goodbye, world!\n"[..])
                                },
                            }),
                        }),
                       &b""[..])));

        assert_eq!(DiffxParser::section(0, Encoding::Binary).parse(&b"\
#diffx: version=1.0,encoding=utf-8
#.foo:
#..bar: content-length=14
Hello, world!

#..baz: content-length=16
Goodbye, world!

#.qux: content-length=0

"[..]),
                   Ok((("diffx",
                        Section {
                            encoding: Encoding::Utf8,
                            options: hashmap!{
                                "version" => "1.0",
                                "encoding" => "utf-8",
                            },
                            content: ChildSections(hashmap!{
                                "foo" => Section {
                                    encoding: Encoding::Utf8,
                                    options: hashmap!{},
                                    content: ChildSections(hashmap!{
                                        "bar" => Section {
                                            encoding: Encoding::Utf8,
                                            options: hashmap!{ "content-length" => "14" },
                                            content: EncodedData("Hello, world!\n"),

                                        },
                                        "baz" => Section {
                                            encoding: Encoding::Utf8,
                                            options: hashmap!{ "content-length" => "16" },
                                            content: EncodedData("Goodbye, world!\n"),

                                        },
                                    }),
                                },
                                "qux" => Section{
                                    encoding: Encoding::Utf8,
                                    options: hashmap!{ "content-length" => "0" },
                                    content: EncodedData(""),
                                },
                            }),
                        }),
                       &b""[..])));

        assert!(DiffxParser::section(0, Encoding::Binary)
            .parse(&b"#diffx: version=1.0,encoding=utf-8\n\n"[..])
            .is_err());
    }
}
