# RadioPost forms

The templates RadioPost publishes for forms sent as messages (RP-2 §8). A form message carries
only its values, by field number; each station holds the template and makes the form's text from
it. A template is named by its fingerprint, the first 8 bytes of SHA-256 of its CBOR encoding, so
a changed template is another template.

| File | Form | Fingerprint |
|---|---|---|
| `ics-213.json` | ICS-213 General Message, after FEMA's public-domain form | `vectors/forms.json` gives it |

Each file is the template as JSON: `name`, an optional `note`, and `fields`, each with its number
`n`, its `label` and `type` (`line`, `text`, `date`, `time`, `number`, `choice`, `check`), and,
where they apply, `required`, `choices`, `bound` (`subject`, `date`, `time`) and `max`. RP-2 §8.1
says how it becomes the CBOR that is fingerprinted.

To add a form, write its template here, add it to `tools/vectors/make.py` so that `vectors/forms.json`
gives its fingerprint, and let the programs hold it. Never change a published template: write a new
one.

These templates are published under CC BY 4.0, as the specifications are.
