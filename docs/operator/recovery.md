# Account recovery

Two commands recover access when the web interface cannot. They run in-process against the data directory, need no Palmr session and have no HTTP equivalent. Their authority is shell access to `/data`.

Both commands change the database, so they refuse to run while a Palmr server is using the data directory. Stop the server first, run the command, then start the server again. `--allow-concurrent` is not accepted.

Every run is recorded in the audit log with the actor `operator_cli`.

## Restore administrator access

```sh
palmr admin recover <user>
```

`<user>` is a user id, an e-mail address or a username, matched in that order and case-insensitively. If one value matches one account's e-mail and another account's username, the e-mail wins. The account must already exist; the command never creates one.

The command makes the account an active Admin:

- promotes it to Admin if it is a regular user;
- reactivates it if it is deactivated;
- clears its login lockout;
- re-enables password login for the instance if it was disabled.

When the account is promoted, every session it had is signed out, so no session created as a regular user gains Admin rights. Running the command on an account that is already an active Admin changes nothing else and is safe to repeat.

```text
Admin recovery completed for user 01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d (ada): the account is an active Admin; role changed: yes, reactivated: no, lockout cleared: yes, password login re-enabled: no, sessions revoked: 2.
```

## Reset a password

```sh
palmr user reset-password <id>
```

`<id>` is the user id, as shown in the Admin user list. E-mail addresses and usernames are not accepted here.

The command replaces the account's password with a new random temporary password and:

- requires the user to choose a new password at the next login;
- signs out every session of the account;
- revokes every trusted device of the account;
- clears the account's login lockout.

It also works for an account that signs in only through an external identity provider: the account gains a local password. It does not re-enable password login for the instance; use `palmr admin recover` for that.

The temporary password is printed once, to standard output, after the reset is saved:

```text
Password reset completed for user 01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d (ada): revoked 2 session(s) and 1 trusted device(s); lockout cleared: yes.
Temporary password: 3q2-7Vx0bQ9mLk4RtY1wZs8uN5cHjD6eFgA_iPoKlMn
The user must change this password at the next login.
```

Deliver it to the user through a channel you trust. Palmr stores only its hash, never writes it to a log, and cannot show it again: if it is lost, run the command again. If the command fails, nothing was changed and no password is printed.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Recovery completed. |
| 2 | The command line is invalid, for example a malformed user id. |
| 67 | No account matches; nothing was changed. |
| 74 | The password was reset but the temporary password could not be written; run the command again. |
| 78 | A Palmr server is using the data directory; stop it first. |
| 1 | The recovery failed and was rolled back; nothing was changed. |
