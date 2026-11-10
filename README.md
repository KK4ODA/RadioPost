# RadioPost

**Email by amateur radio, with no central server.**

RadioPost lets licensed amateurs send and receive ordinary email over HF and VHF radio. Anyone
licensed can run a **gateway**; clubs and groups run **post offices**; addresses are ordinary email
addresses (`name@radiopost.org`), so anyone with Gmail or Outlook can write to a RadioPost user
and get an answer. When the Internet fails, gateways keep checking users, holding their mail and
queueing what they send, and later forward it to each other by radio.

It is **federated**: many post offices and gateways, run by different people, work as one network
because they follow the same published protocols, as email providers do.

## Start here

| | |
|---|---|
| [The concept](docs/concept/00-concept.md) | What RadioPost is and how it works, in plain language, with schematics. |
| [The detailed design](docs/design/00-detailed-design.md) | The programs, the connections between them, and the decisions behind them. |
| [The specifications](spec/README.md) | The protocols: what every client, gateway, post office and modem adapter implements. |

## Build your own

The protocols are the contract, not the programs. The RadioPost client, gateway and post office
service are one implementation each; you may use them, or write your own client, gateway, post
office or modem adapter from the specifications, and it will work with everyone else's.

| You are building | Start with |
|---|---|
| A client | RP-1, RP-2, RP-3 |
| A gateway | RP-1, RP-2, RP-3, RP-5 |
| A post office | RP-1, RP-2, RP-5, RP-6 |
| An adapter for a modem | RP-4 |

## Status

Phases 1 and 2 are built: the core library in [`core/`](core/README.md) builds RadioPost's objects
and runs RP-3's sessions, checked against the test vectors, and its modem interface (RP-4) carries
them through [Aether HF](https://github.com/KK4ODA/aether-hf) or a simulated channel. The
`radiopost` client carries mail to another station or through a gateway, on the command line, from
its panel in a browser, or as a desktop application ([`app/client`](app/client/README.md)); the
gateway answers users' calls and carries their sessions to their post offices, as a desktop
application of its own, RadioPost Gateway ([`app/gateway`](app/gateway/README.md),
[the operator's guide](docs/user/gateway.md)); and the post office service runs beside a mail
server and exchanges ordinary email ([the operator's guide](docs/user/post-office.md)).
On-air tests can begin: [between two stations](docs/user/on-air-tests.md), and
[email through a gateway](docs/user/on-air-gateway.md) once `radiopost.org` is live. The specifications are **drafts**:
anything may change, and nothing is ready to deploy. The project's own post office will be `radiopost.org`. The first modem is
[Aether HF](https://github.com/KK4ODA/aether-hf), an open-source HF modem; VARA, ARDOP and AX.25
packet follow through adapters.

## Taking part

Questions, ideas and corrections are welcome as issues. Changes to the specifications go through
pull requests, as [`CONTRIBUTING.md`](CONTRIBUTING.md) describes.

## Licence

| What | Licence |
|---|---|
| The specifications (`spec/`), the documents (`docs/`) and their figures | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) (`LICENSE-CC-BY-4.0`) |
| Code (`tools/`, and the programs to come) | MIT (`LICENSE-MIT`) or Apache 2.0 (`LICENSE-APACHE`), at your option |

Anyone may implement the protocols, in open or closed software, without asking; please credit the
RadioPost project.
