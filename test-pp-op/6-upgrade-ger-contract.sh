package test_pp_op


set -e
set -x
source .env
source ./utils.sh

cast send $TIMELOCK_OVERRIDE_PROPOSER_ADDRESS --value 1ether --from $DEPLOYER_ADDRESS --private-key $DEPLOYER_PRIVATE_KEY --rpc-url $L2_SEQ_URL
cast send $TIMELOCK_OVERRIDE_EXECUTOR_ADDRESS --value 1ether --from $DEPLOYER_ADDRESS --private-key $DEPLOYER_PRIVATE_KEY --rpc-url $L2_SEQ_URL


cd $TMP_DIR/xlayer-contracts/

cd upgrade/upgradeToV2

echo "Creating upgrade_parameters.json..."
cat > upgrade_parameters.json << EOF
{
    "timelockDelay": $TIME_LOCK_DELAY,
    "timelockSalt": "",
    "globalExitRootUpdater": "$ORACLE_ADDRESS",
    "globalExitRootRemover": "$ORACLE_ADDRESS"
}
EOF

cp ../../deployment/v2/deploy_parameters.json ./deploy_parameters.json
cp ../../deployment/v2/deploy_output.json ./deploy_output.json

sed_inplace '1s/{/{\n "polygonZkEVMGlobalExitRootL2Address": "0xa40d5f56745a118d0906a34e69aec8c0db1cb8fa",/' deploy_output.json

cd ../../

hardhat_output=$(npm run upgradeL2GER:timelock:l2localhost)
echo "hardhat_output: $hardhat_output"

schedule_data=$(echo "$hardhat_output" | awk -F"'" '/scheduleData:/ {print $2}')
execute_data=$(echo "$hardhat_output" | awk -F"'" '/executeData:/ {print $2}')
echo "schedule_data: $schedule_data"
echo "execute_data: $execute_data"

cast send --rpc-url "$L2_SEQ_URL" -f $TIMELOCK_OVERRIDE_PROPOSER_ADDRESS --private-key "$TIMELOCK_OVERRIDE_PROPOSER_PRIVATE_KEY" "$TIME_LOCK_ADDRESS" "$schedule_data"

sleep $TIME_LOCK_DELAY
cast send --rpc-url "$L2_SEQ_URL" -f $TIMELOCK_OVERRIDE_EXECUTOR_ADDRESS --private-key "$TIMELOCK_OVERRIDE_EXECUTOR_PRIVATE_KEY" "$TIME_LOCK_ADDRESS" "$execute_data"
sleep 5
cast call --rpc-url "$L2_SEQ_URL" $GER_MANAGER_ADDRESS 'GER_SOVEREIGN_VERSION()(string)'
