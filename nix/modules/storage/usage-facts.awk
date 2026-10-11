# usage.tsv rows ("<member>\t<bytes>") to heartbeat facts: usage/<svc>/<member> <bytes>
{
    m = $1
    if (suffix != "" && substr(m, length(m) - length(suffix) + 1) == suffix)
        m = substr(m, 1, length(m) - length(suffix))
}
m ~ /^[A-Za-z0-9._-]+$/ && $2 ~ /^[0-9]+$/ { printf "usage/%s/%s %s\n", svc, m, $2 }
