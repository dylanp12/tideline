"""The span reader — the piece that makes verification possible in Python."""

import pytest

from tideline import ParseError, parse_event, parse_record, raw_member, split_array


def test_returns_the_values_exact_text():
    assert raw_member('{"a":1,"metadata":{ "z" : 1 }}', "metadata") == '{ "z" : 1 }'


def test_picks_the_top_level_member_not_a_nested_one():
    # A substring search finds the nested key first and hashes the wrong bytes.
    src = '{"seq":1,"redacted":{"metadata":"aa"},"metadata":{"real":true}}'
    assert raw_member(src, "metadata") == '{"real":true}'


def test_is_not_fooled_by_the_key_name_inside_a_string():
    src = '{"content":"see \\"metadata\\": below","metadata":{"real":1}}'
    assert raw_member(src, "metadata") == '{"real":1}'


def test_absent_member_is_none():
    assert raw_member('{"a":1}', "metadata") is None
    assert raw_member("{}", "metadata") is None


@pytest.mark.parametrize(
    "key,expected",
    [
        ("s", '"x"'),
        ("n", "-1.5e3"),
        ("t", "true"),
        ("z", "null"),
        ("a", '[1,{"b":2}]'),
        ("o", '{"c":[]}'),
    ],
)
def test_handles_every_value_shape(key, expected):
    src = '{"s":"x","n":-1.5e3,"t":true,"z":null,"a":[1,{"b":2}],"o":{"c":[]}}'
    assert raw_member(src, key) == expected


def test_rejects_malformed_input_rather_than_guessing():
    with pytest.raises(ParseError):
        raw_member('{"a"', "a")
    with pytest.raises(ParseError):
        raw_member("not an object", "a")


def test_split_array_handles_brackets_inside_strings():
    assert split_array('[{"c":"]}["},{"c":"b"}]') == ['{"c":"]}["}', '{"c":"b"}']


def test_split_array_handles_empty():
    assert split_array("[]") == []
    assert split_array("  [ ]  ") == []


def test_parse_event_keeps_bytes_and_exposes_a_value():
    e = parse_event('{"seq":0,"ts":1,"kind":"run_started","metadata":{ "a" : 1 }}')
    assert e.metadata_raw == '{ "a" : 1 }'
    assert e.metadata == {"a": 1}


def test_null_metadata_is_absent():
    # Absent and null are the same claim: no metadata. An empty digest, not a
    # digest of the four characters "null".
    e = parse_event('{"seq":0,"ts":1,"kind":"run_started","metadata":null}')
    assert e.metadata_raw is None


def test_unknown_kind_is_rejected():
    with pytest.raises(ParseError):
        parse_event('{"seq":0,"ts":1,"kind":"wire_transfer"}')


def test_parse_record_round_trips():
    text = (
        '[{"seq":0,"ts":1,"kind":"run_started","metadata":{"a":1}},'
        '{"seq":1,"ts":2,"kind":"message","content":"hi"}]'
    )
    events = parse_record(text)
    assert len(events) == 2
    assert events[0].metadata_raw == '{"a":1}'
    assert events[1].content == "hi"
