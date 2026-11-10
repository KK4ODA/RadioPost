# RadioPost

**Email by amateur radio, with no central server.**

RadioPost lets licensed amateurs send and receive ordinary email over HF and VHF radio. Anyone
licensed can run a **gateway**; clubs and groups run **post offices**; addresses are ordinary email
addresses (`name@radiopost.org`), so anyone with Gmail or Outlook can write to a RadioPost user
and get an answer. When the Internet fails, gateways keep checking users, holding their mail and
queueing what they send, and later forward it to each other by radio.

It is **federated**: many post offices and gateways, run by different people, work as one network
because they follow the same published protocols, as email providers do.

## What it is for

**RadioPost is not meant to replace Winlink.** Winlink works: it has hundreds of gateways, carries
email both ways, and is used in emergencies. RadioPost does not speak Winlink's protocol and does
not ask anyone to leave it. A station can run both, on one radio.

RadioPost is a place for amateurs to **experiment and build**. Its protocols are published, with
test vectors and a conformance tester, so that anyone can write a program that works with everyone
else's:

* Write a client, a gateway or a post office of your own, or change ours. The code is open.
* Put a new modem under it with an adapter. Nothing above the modem interface changes.
* Build an application on top of it: forms, telemetry, a bulletin service, a bridge to something
  else. A message is addressed and delivered the same way whatever it carries.
* Run a post office for your club, on your own domain, with your own rules. Nobody's permission is
  needed.

The common protocol is the point. The programs here are one implementation of it, with no special
standing.

## Start here

| | |
|---|---|
| [The concept](docs/concept/00-concept.md) | What RadioPost is and how it works, in plain language, with schematics. |
| [The detailed design](docs/design/00-detailed-design.md) | The programs, the connections between them, and the decisions behind them. |
| [The specifications](spec/README.md) | The protocols: what every client, gateway, post office and modem adapter implements. |
| [The test vectors](vectors/README.md) | Exact inputs and outputs for the protocols, to check an implementation against. |
| [The conformance tester's outline](docs/design/01-conformance.md) | What the tester will check of each role, and what each phase adds to it. |

## Build your own

The protocols are the contract, not the programs. The RadioPost client, gateway and post office
service are one implementation each; you may use them, or write your own client, gateway, post
office or modem adapter from the specifications, and it will work with everyone else's.

| You are building | Start with |
|---|---|
| A client | RP-1, RP-2, RP-3; RP-4 to drive a modem |
| A gateway | RP-1, RP-2, RP-3, RP-5; RP-4 to drive modems; RP-7 to forward when cut off |
| A post office | RP-1, RP-2, RP-5, RP-6 |
| An adapter for a modem | RP-4 |

RP-0 holds the conventions they all share. [The index](spec/README.md) says which parts of each
apply. The [test vectors](vectors/README.md) give the exact bytes and verdicts: keys, credentials,
messages, every PDU, whole sessions byte for byte, the modem interface's conversations, the post
office's exchanges, email in and out, and forwarding. Check an implementation against them before
it goes on the air.

## Status

The specifications are **drafts**: anything may change, and nothing is ready to deploy. All eight
(RP-0 to RP-7) are written, with test vectors for RP-1 to RP-7.

The programs follow the [order of work](docs/design/00-detailed-design.md#9-order-of-work), in
Rust, each checked against the test vectors:

* **The core library and the modem interface** ([`core/`](core/README.md)): RadioPost's objects,
  RP-3's sessions, and RP-4 carrying them between two stations.
* **Two adapters** (`adapters/`): one for [Aether HF](https://github.com/KK4ODA/aether-hf), an
  open-source HF modem, and one for TCP. VARA, ARDOP and AX.25 packet follow.
* **The client** ([`app/client`](app/client/README.md)): mail between two stations, on the command
  line, from its panel in a browser, or as a desktop application.
  [On-air tests](docs/user/on-air-tests.md) between two stations with Aether HF can begin.
* **The gateway and the post office service**, online, with ordinary email both ways
  ([running a post office](docs/user/post-office.md)). The project's own post office will be
  `radiopost.org`; it is not live yet.
* **The conformance tester** ([its outline](docs/design/01-conformance.md)), which checks a client,
  a gateway, a post office or an adapter, whoever wrote it.

Still to come: copies placed ahead and the offline queue, then forwarding by radio.

## In this repository

| | |
|---|---|
| [`spec/`](spec/README.md) | The specifications. Where a program and a specification disagree, the specification wins. |
| [`docs/`](docs/) | The concept, the detailed design and the user guides, with their figures. |
| [`vectors/`](vectors/README.md) | The test vectors, generated by `tools/vectors/make.py`, never edited by hand. |
| [`core/`](core/README.md) | The Rust workspace: the core library, the modem interface, the gateway, the post office service, the conformance tester. |
| `adapters/` | The modem adapters: Aether HF and TCP. |
| [`app/`](app/client/README.md) | The client: the `radiopost` program, its panel and the desktop application. |
| [`tools/`](tools/) | The vectors' reference, the figures' generators and the documents' checker (Python, standard library only). |

## Taking part

Questions, ideas and corrections are welcome as issues. Changes to the specifications go through
pull requests, as [`CONTRIBUTING.md`](CONTRIBUTING.md) describes.

## Licence

| What | Licence |
|---|---|
| The specifications (`spec/`), the documents (`docs/`) and their figures | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) (`LICENSE-CC-BY-4.0`) |
| Code (`core/`, `adapters/`, `app/`, `tools/`) | MIT (`LICENSE-MIT`) or Apache 2.0 (`LICENSE-APACHE`), at your option |

Anyone may implement the protocols, in open or closed software, without asking; please credit the
RadioPost project.
