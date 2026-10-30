// Local synthetic test only. Exit after the committed packet without running
// JS finalizers or explicitly closing SQLite, so the parent can test recovery.
import { DatabaseSync } from "node:sqlite";
import { readFileSync, writeSync } from "node:fs";
import { JournaledRoomClient } from "../src/journal.mjs";
import { SqliteRoomJournal } from "../src/sqlite-journal.mjs";

const { identity, snapshot } = JSON.parse(readFileSync(0, "utf8"));
const database = new DatabaseSync(process.argv[2]), journal = new SqliteRoomJournal(database, identity);
const client = await JournaledRoomClient.open(identity, journal), connection = client.beginConnection();
client.receiveSnapshot(connection, snapshot);
const packet = await client.outbound();
if (!packet) throw new Error("Synthetic test expected a committed packet");
writeSync(1, JSON.stringify(packet));
process.exit(73);
