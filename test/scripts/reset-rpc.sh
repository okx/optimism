docker compose down op-rpc
docker compose down op-geth-rpc

cd data

rm -rf op-rpc
rm -rf op-geth-rpc
cp -r op-geth-rpc-bak op-geth-rpc
