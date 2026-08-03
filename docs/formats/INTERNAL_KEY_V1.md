# Internal Key V1

Internal Key V1 is a bounded canonical encoding for one logical internal key. Serialized byte order
is **not** the logical comparator order.

```text
offset  size  field
0       4     magic = "FIKY"
4       1     version = 1
5       3     reserved, zero
8       4     column family ID, little-endian
12      4     user-key length, little-endian
16      N     user-key bytes
16+N    8     sequence number, little-endian
24+N    1     value type (0 deletion, 1 value)
```

Readers reject wrong magic, unsupported versions, non-zero reserved bytes, empty keys, oversized
keys, sequence zero, unknown value types, integer overflow, truncation, and trailing bytes.

Logical comparison is:

1. column family ascending;
2. user key lexicographically ascending;
3. sequence descending;
4. value type descending.
