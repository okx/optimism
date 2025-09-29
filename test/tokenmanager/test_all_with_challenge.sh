cd contracts
npm install

cast send 0xDE282DC882bbB5100b8A24E30D38a2D5B3080c15 --value 10ether --private-key 0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2 --rpc-url http://127.0.0.1:8123

cd ..

./deploy_tokenmanager.sh

./test_tokenmanager.sh
