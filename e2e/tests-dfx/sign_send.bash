#!/usr/bin/env bats

load ../utils/_

setup() {
    # We want to work from a temporary directory, different for every test.
    cd "$(mktemp -d -t dfx-e2e-XXXXXXXX)" || exit
    export RUST_BACKTRACE=1

    dfx_new hello
}

teardown() {
  dfx_stop
}

@test "sign + send" {
    install_asset counter
    dfx_start
    dfx deploy --no-wallet

    assert_command dfx canister --no-wallet sign --query hello read
    assert_eq "Query message generated at [message.json]"

    sleep 10
    echo y | assert_command dfx canister --no-wallet send message.json

    assert_command_fail dfx canister --no-wallet sign --query hello read
    assert_eq "[message.json] already exists, please specify a different output file name."

    assert_command dfx canister --no-wallet sign --update hello inc --file message-inc.json
    assert_eq "Update message generated at [message-inc.json]"

    sleep 10
    echo y | assert_command dfx canister --no-wallet send message-inc.json
}
