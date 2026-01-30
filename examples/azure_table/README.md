# Azure Table Storage Example

This is a small CLI that invokes the `azure_table` backend directly so that some desk testing can be done.

It requires environment variables:

```
AZURE_STORAGE_ACCOUNT=<storage account name>
AZURE_STORAGE_KEY=<base64 encoded access key>
```

See the code for the `Customer` struct that should be a match to the data stored in a table named `testAugenticBE`.

Sample data in the table should be of the form:

| Property Name      | Type               | Example Value                     |
|--------------------|--------------------|-----------------------------------|
| PartitionKey       | String             | testAugenticBE                    |
| RowKey             | String             | yrgp8tsmxwlc5jh0ovd19wqn          |
| Timestamp          | DateTime           | (system generated)                |
| Id                 | String             | yrgp8tsmxwlc5jh0ovd19wqn          |
| Name               | String             | Alice Montgomery                  |
| IsActive           | Boolean            | true                              |
| Created            | DateTime           | 2013-08-09T18:55:48.3402073Z      |
| Points             | Int32              | 102                               |
| Discount           | Double             | 0.125                             |
| Avatar             | Binary             | VGVzdGluZy0xMjM=                  |

Enter the following values that match the hard-coded queries in the example (PartitionKey is `testAugenticBE` and RowKey is the same as Id):

| Id                       | Name             | IsActive | Created                      | Points | Discount | Avatar           |
|--------------------------|------------------|----------|------------------------------|--------|----------|------------------|
| yrgp8tsmxwlc5jh0ovd19wqn | Alice Montgomery | true     | 2013-08-09T18:55:48.3402073Z | 102    | 0.125    | VGVzdGluZy0xMjM= |
| g6fw3pouvk2hffs4bvs3kl0z | Bob Burns        | false    | 2025-10-20T06:23Z            |  56    | 0        |                  |
| aru4fkdqjk7gv1ojdfiz0b55 | Eve Dropper      | true     | 2026-01-30T01:32:32Z         |  10    | 0.1      | QW5vdGhlciBUZXN0 |

Run the example using

```bash
cargo run --example azure_table
```
