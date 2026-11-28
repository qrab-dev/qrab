# Test fixtures

`secureboot-test-cert.der` is a self-signed X.509 certificate
(`CN=qrab-secureboot-test`, RSA 2048, SHA-256, valid 2026-11-14 to
2036-11-11) that the Secure Boot tests parse as the contents of a signature
database entry. It is test data only: its private key is not in this
repository, and it signs nothing. The tests read it with
`include_bytes!`. An equivalent certificate can be made with

```sh
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
    -subj /CN=qrab-secureboot-test -keyout /dev/null -outform der \
    -out secureboot-test-cert.der
```
