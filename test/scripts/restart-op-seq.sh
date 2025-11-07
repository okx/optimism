source .env

docker compose down op-seq

# sleep time is important!
sleep 10

docker compose up -d op-seq
