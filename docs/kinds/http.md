url must be an absolute https URL; method and url are the only required fields -- args_schema is inferred from the url/header/body templates when omitted.

```json
{
  "method": "GET",
  "url": "https://api.example.com/items/{{id}}"
}
```

Call arguments:

```json
{"id": "123"}
```
