import assert from 'node:assert/strict';
import test from 'node:test';
import { generate } from './generate-api.mjs';

test('preserves requiredness, nullability, enums, references, and unknown fields', () => {
  const source = generate({ example: { type: 'object', required: ['link', 'unknown'], properties: {
    link: { type: 'string', nullable: true },
    unknown: { description: 'The converter has not verified this type' },
    optional_link: { type: 'string', nullable: true },
    state: { type: 'string', enum: ['Needs review', 'Merged'] },
    children: { type: 'array', items: { $ref: '#/components/schemas/example' } },
  } } });
  assert.match(source, /pub link: Nullable<String>/);
  assert.match(source, /pub unknown: Value/);
  assert.match(source, /pub optional_link: Option<Nullable<String>>/);
  assert.match(source, /pub children: Option<Vec<Example>>/);
  assert.match(source, /serde\(rename = "Needs review"\)/);
  assert.match(source, /deserialize_with = "required"/);
  assert.match(source, /deserialize_with = "present"/);
});

test('a nullable object does not rename the field enum types', () => {
  const source = generate({ example: { type: 'object', nullable: true, properties: { state: { type: 'string', enum: ['Active'] } } } });
  assert.match(source, /pub struct ExampleObject/);
  assert.match(source, /pub type Example = Nullable<ExampleObject>/);
  assert.match(source, /pub state: Option<ExampleState>/);
  assert.doesNotMatch(source, /ExampleObjectState/);
});

test('fails closed rather than guessing unsupported types or references', () => {
  assert.throws(() => generate({ bad: { type: 'invented' } }), /Unsupported schema/);
  assert.throws(() => generate({ bad: { $ref: '#/components/schemas/missing' } }), /Unknown reference/);
});
