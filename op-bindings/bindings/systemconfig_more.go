// Code generated - DO NOT EDIT.
// This file is a generated binding and any manual changes will be lost.

package bindings

import (
	"encoding/json"

	"github.com/ethereum-optimism/optimism/op-bindings/solc"
)

const SystemConfigStorageLayoutJSON = "{\"storage\":[{\"astId\":1000,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"_initialized\",\"offset\":0,\"slot\":\"0\",\"type\":\"t_uint8\"},{\"astId\":1001,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"_initializing\",\"offset\":1,\"slot\":\"0\",\"type\":\"t_bool\"},{\"astId\":1002,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"__gap\",\"offset\":0,\"slot\":\"1\",\"type\":\"t_array(t_uint256)50_storage\"},{\"astId\":1003,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"_owner\",\"offset\":0,\"slot\":\"51\",\"type\":\"t_address\"},{\"astId\":1004,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"__gap\",\"offset\":0,\"slot\":\"52\",\"type\":\"t_array(t_uint256)49_storage\"},{\"astId\":1005,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"overhead\",\"offset\":0,\"slot\":\"101\",\"type\":\"t_uint256\"},{\"astId\":1006,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"scalar\",\"offset\":0,\"slot\":\"102\",\"type\":\"t_uint256\"},{\"astId\":1007,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"batcherHash\",\"offset\":0,\"slot\":\"103\",\"type\":\"t_bytes32\"},{\"astId\":1008,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"gasLimit\",\"offset\":0,\"slot\":\"104\",\"type\":\"t_uint64\"},{\"astId\":1009,\"contract\":\"src/L1/SystemConfig.sol:SystemConfig\",\"label\":\"_resourceConfig\",\"offset\":0,\"slot\":\"105\",\"type\":\"t_struct(ResourceConfig)1010_storage\"}],\"types\":{\"t_address\":{\"encoding\":\"inplace\",\"label\":\"address\",\"numberOfBytes\":\"20\"},\"t_array(t_uint256)49_storage\":{\"encoding\":\"inplace\",\"label\":\"uint256[49]\",\"numberOfBytes\":\"1568\",\"base\":\"t_uint256\"},\"t_array(t_uint256)50_storage\":{\"encoding\":\"inplace\",\"label\":\"uint256[50]\",\"numberOfBytes\":\"1600\",\"base\":\"t_uint256\"},\"t_bool\":{\"encoding\":\"inplace\",\"label\":\"bool\",\"numberOfBytes\":\"1\"},\"t_bytes32\":{\"encoding\":\"inplace\",\"label\":\"bytes32\",\"numberOfBytes\":\"32\"},\"t_struct(ResourceConfig)1010_storage\":{\"encoding\":\"inplace\",\"label\":\"struct ResourceMetering.ResourceConfig\",\"numberOfBytes\":\"32\"},\"t_uint128\":{\"encoding\":\"inplace\",\"label\":\"uint128\",\"numberOfBytes\":\"16\"},\"t_uint256\":{\"encoding\":\"inplace\",\"label\":\"uint256\",\"numberOfBytes\":\"32\"},\"t_uint32\":{\"encoding\":\"inplace\",\"label\":\"uint32\",\"numberOfBytes\":\"4\"},\"t_uint64\":{\"encoding\":\"inplace\",\"label\":\"uint64\",\"numberOfBytes\":\"8\"},\"t_uint8\":{\"encoding\":\"inplace\",\"label\":\"uint8\",\"numberOfBytes\":\"1\"}}}"

var SystemConfigStorageLayout = new(solc.StorageLayout)

var SystemConfigDeployedBin = "0x608060405234801561001057600080fd5b50600436106102ad5760003560e01c80638da5cb5b1161017b578063d8444715116100d8578063f45e65d81161008c578063f8c68de011610071578063f8c68de014610661578063fd32aa0f14610669578063ffa1ad741461067157600080fd5b8063f45e65d814610644578063f68016b71461064d57600080fd5b8063e0e2016d116100bd578063e0e2016d14610620578063e81b2c6d14610628578063f2fde38b1461063157600080fd5b8063d844471514610610578063dac6e63a1461061857600080fd5b8063bc49ce5f1161012f578063c71973f611610114578063c71973f6146104b6578063c9b26f61146104c9578063cc731b02146104dc57600080fd5b8063bc49ce5f146104a6578063c4e8ddfa146104ae57600080fd5b80639b7d7f0a116101605780639b7d7f0a14610483578063a71198691461048b578063b40a817c1461049357600080fd5b80638da5cb5b14610452578063935f029e1461047057600080fd5b806348cd4cb11161022957806354fd4d50116101dd5780635d73369c116101c25780635d73369c1461043a57806361d1576814610442578063715018a61461044a57600080fd5b806354fd4d50146103e9578063550fcdc91461043257600080fd5b80634c1e843d1161020e5780634c1e843d146103a75780634d9f1559146103ba5780634f16540b146103c257600080fd5b806348cd4cb11461037e5780634add321d1461038657600080fd5b806318d13918116102805780631fd19ee1116102655780631fd19ee11461032857806321326849146103305780634397dfef1461034857600080fd5b806318d139181461030b57806319f5cea81461032057600080fd5b806306c92657146102b2578063078f29cf146102cd5780630a49cb03146102fa5780630c18c16214610302575b600080fd5b6102ba610679565b6040519081526020015b60405180910390f35b6102d56106a7565b60405173ffffffffffffffffffffffffffffffffffffffff90911681526020016102c4565b6102d56106e0565b6102ba60655481565b61031e610319366004611fc4565b610710565b005b6102ba610724565b6102d561074f565b610338610779565b60405190151581526020016102c4565b6103506107b8565b6040805173ffffffffffffffffffffffffffffffffffffffff909316835260ff9091166020830152016102c4565b6102ba6107cc565b61038e6107fc565b60405167ffffffffffffffff90911681526020016102c4565b61031e6103b5366004612137565b610822565b6102d5610c2d565b6102ba7f65a7ed542fb37fe237fdfbdd70b31598523fe5b32879e307bae27a0bd9581c0881565b6104256040518060400160405280601c81526020017f312e31332e302d626574612b637573746f6d2d6761732d746f6b656e0000000081525081565b6040516102c491906122ec565b610425610c5d565b6102ba610c67565b6102ba610c92565b61031e610cbd565b60335473ffffffffffffffffffffffffffffffffffffffff166102d5565b61031e61047e3660046122ff565b610cd1565b6102d5610ce7565b6102d5610d17565b61031e6104a1366004612321565b610d47565b6102ba610d58565b6102d5610d83565b61031e6104c436600461233c565b610db3565b61031e6104d7366004612358565b610dc4565b6105a06040805160c081018252600080825260208201819052918101829052606081018290526080810182905260a0810191909152506040805160c08101825260695463ffffffff8082168352640100000000820460ff9081166020850152650100000000008304169383019390935266010000000000008104831660608301526a0100000000000000000000810490921660808201526e0100000000000000000000000000009091046fffffffffffffffffffffffffffffffff1660a082015290565b6040516102c49190600060c08201905063ffffffff80845116835260ff602085015116602084015260ff6040850151166040840152806060850151166060840152806080850151166080840152506fffffffffffffffffffffffffffffffff60a08401511660a083015292915050565b610425610dd5565b6102d5610ddf565b6102ba610e0f565b6102ba60675481565b61031e61063f366004611fc4565b610e3a565b6102ba60665481565b60685461038e9067ffffffffffffffff1681565b6102ba610eee565b6102ba610f19565b6102ba600081565b6106a460017fa04c5bb938ca6fc46d95553abf0a76345ce3e722a30bf4f74928b8e7d852320d6123a0565b81565b60006106db6106d760017f9904ba90dde5696cda05c9e0dab5cbaa0fea005ace4d11218a02ac668dad63776123a0565b5490565b905090565b60006106db6106d760017f4b6c74f9e688cb39801f2112c14a8c57232a3fc5202e1444126d4bce86eb19ad6123a0565b61071861117f565b61072181611200565b50565b6106a460017f46adcbebc6be8ce551740c29c47c8798210f23f7f4086c41752944352568d5a86123a0565b60006106db7f65a7ed542fb37fe237fdfbdd70b31598523fe5b32879e307bae27a0bd9581c085490565b6000806107846107b8565b5073ffffffffffffffffffffffffffffffffffffffff1673eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee141592915050565b6000806107c3611102565b90939092509050565b60006106db6106d760017fa11ee3ab75b40e88a0105e935d17cd36c8faee0138320d776c411291bdbbb1a06123a0565b6069546000906106db9063ffffffff6a01000000000000000000008204811691166123b7565b600054610100900460ff16158080156108425750600054600160ff909116105b8061085c5750303b15801561085c575060005460ff166001145b6108ed576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602e60248201527f496e697469616c697a61626c653a20636f6e747261637420697320616c72656160448201527f647920696e697469616c697a656400000000000000000000000000000000000060648201526084015b60405180910390fd5b600080547fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00166001179055801561094b57600080547fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00ff166101001790555b6109536112bd565b61095c8a610e3a565b6109658761135c565b61096f8989611384565b61097886611415565b6109a17f65a7ed542fb37fe237fdfbdd70b31598523fe5b32879e307bae27a0bd9581c08869055565b6109d46109cf60017f71ac12829d66ee73d8d95bff50b3589745ce57edae70a3fb111a2342464dc5986123a0565b849055565b610a08610a0260017f383f291819e6d54073bc9a648251d97421076bdd101933c0c022219ce95806376123a0565b83519055565b610a3f610a3660017f46adcbebc6be8ce551740c29c47c8798210f23f7f4086c41752944352568d5a86123a0565b60208401519055565b610a76610a6d60017f9904ba90dde5696cda05c9e0dab5cbaa0fea005ace4d11218a02ac668dad63776123a0565b60408401519055565b610aad610aa460017fe52a667f71ec761b9b381c7b76ca9b852adf7e8905da0e0ad49986a0a68718166123a0565b60608401519055565b610ae4610adb60017f4b6c74f9e688cb39801f2112c14a8c57232a3fc5202e1444126d4bce86eb19ad6123a0565b60808401519055565b610b1b610b1260017fa04c5bb938ca6fc46d95553abf0a76345ce3e722a30bf4f74928b8e7d852320d6123a0565b60a08401519055565b610b236114f3565b610b308260c0015161155b565b610b3984611865565b610b416107fc565b67ffffffffffffffff168667ffffffffffffffff161015610bbe576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152601f60248201527f53797374656d436f6e6669673a20676173206c696d697420746f6f206c6f770060448201526064016108e4565b8015610c2157600080547fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00ff169055604051600181527f7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb38474024989060200160405180910390a15b50505050505050505050565b60006106db6106d760017fe52a667f71ec761b9b381c7b76ca9b852adf7e8905da0e0ad49986a0a68718166123a0565b60606106db611cd9565b6106a460017f383f291819e6d54073bc9a648251d97421076bdd101933c0c022219ce95806376123a0565b6106a460017fe52a667f71ec761b9b381c7b76ca9b852adf7e8905da0e0ad49986a0a68718166123a0565b610cc561117f565b610ccf6000611d9a565b565b610cd961117f565b610ce38282611384565b5050565b60006106db6106d760017fa04c5bb938ca6fc46d95553abf0a76345ce3e722a30bf4f74928b8e7d852320d6123a0565b60006106db6106d760017f383f291819e6d54073bc9a648251d97421076bdd101933c0c022219ce95806376123a0565b610d4f61117f565b61072181611415565b6106a460017f71ac12829d66ee73d8d95bff50b3589745ce57edae70a3fb111a2342464dc5986123a0565b60006106db6106d760017f46adcbebc6be8ce551740c29c47c8798210f23f7f4086c41752944352568d5a86123a0565b610dbb61117f565b61072181611865565b610dcc61117f565b6107218161135c565b60606106db611e11565b60006106db6106d760017f71ac12829d66ee73d8d95bff50b3589745ce57edae70a3fb111a2342464dc5986123a0565b6106a460017fa11ee3ab75b40e88a0105e935d17cd36c8faee0138320d776c411291bdbbb1a06123a0565b610e4261117f565b73ffffffffffffffffffffffffffffffffffffffff8116610ee5576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602660248201527f4f776e61626c653a206e6577206f776e657220697320746865207a65726f206160448201527f646472657373000000000000000000000000000000000000000000000000000060648201526084016108e4565b61072181611d9a565b6106a460017f9904ba90dde5696cda05c9e0dab5cbaa0fea005ace4d11218a02ac668dad63776123a0565b6106a460017f4b6c74f9e688cb39801f2112c14a8c57232a3fc5202e1444126d4bce86eb19ad6123a0565b9055565b73ffffffffffffffffffffffffffffffffffffffff163b151590565b6000602082511115610ff8576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152603660248201527f476173506179696e67546f6b656e3a20737472696e672063616e6e6f7420626560448201527f2067726561746572207468616e2033322062797465730000000000000000000060648201526084016108e4565b611001826110d9565b92915050565b61106d61103560017f04adb1412b2ddc16fcc0d4538d5c8f07cf9c83abecc6b41f6f69037b708fbcec6123a0565b74ff000000000000000000000000000000000000000060a086901b1673ffffffffffffffffffffffffffffffffffffffff8716179055565b6110a061109b60017f657c3582c29b3176614e3a33ddd1ec48352696a04e92b3c0566d72010fa8863d6123a0565b839055565b6110d36110ce60017fa48b38a4b44951360fbdcbfaaeae5ed6ae92585412e9841b70ec72ed8cd057646123a0565b829055565b50505050565b8051602181106110f15763ec92f9a36000526004601cfd5b9081015160209190910360031b1b90565b600080806111346106d760017f04adb1412b2ddc16fcc0d4538d5c8f07cf9c83abecc6b41f6f69037b708fbcec6123a0565b73ffffffffffffffffffffffffffffffffffffffff81169350905082611173575073eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee92601292509050565b60a081901c9150509091565b60335473ffffffffffffffffffffffffffffffffffffffff163314610ccf576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820181905260248201527f4f776e61626c653a2063616c6c6572206973206e6f7420746865206f776e657260448201526064016108e4565b6112297f65a7ed542fb37fe237fdfbdd70b31598523fe5b32879e307bae27a0bd9581c08829055565b6040805173ffffffffffffffffffffffffffffffffffffffff8316602082015260009101604080517fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe0818403018152919052905060035b60007f1d2b0bda21d56b8bd12d4f94ebacffdfb35f5e226f84b461103bb8beab6353be836040516112b191906122ec565b60405180910390a35050565b600054610100900460ff16611354576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602b60248201527f496e697469616c697a61626c653a20636f6e7472616374206973206e6f74206960448201527f6e697469616c697a696e6700000000000000000000000000000000000000000060648201526084016108e4565b610ccf611ec7565b6067819055604080516020808201849052825180830390910181529082019091526000611280565b606582905560668190556040805160208101849052908101829052600090606001604080517fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe08184030181529190529050600160007f1d2b0bda21d56b8bd12d4f94ebacffdfb35f5e226f84b461103bb8beab6353be8360405161140891906122ec565b60405180910390a3505050565b61141d6107fc565b67ffffffffffffffff168167ffffffffffffffff16101561149a576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152601f60248201527f53797374656d436f6e6669673a20676173206c696d697420746f6f206c6f770060448201526064016108e4565b606880547fffffffffffffffffffffffffffffffffffffffffffffffff00000000000000001667ffffffffffffffff83169081179091556040805160208082019390935281518082039093018352810190526002611280565b6115216106d760017fa11ee3ab75b40e88a0105e935d17cd36c8faee0138320d776c411291bdbbb1a06123a0565b600003610ccf57610ccf61155660017fa11ee3ab75b40e88a0105e935d17cd36c8faee0138320d776c411291bdbbb1a06123a0565b439055565b73ffffffffffffffffffffffffffffffffffffffff8116158015906115aa575073ffffffffffffffffffffffffffffffffffffffff811673eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee14155b80156115bb57506115b9610779565b155b1561072157601260ff168173ffffffffffffffffffffffffffffffffffffffff1663313ce5676040518163ffffffff1660e01b8152600401602060405180830381865afa158015611610573d6000803e3d6000fd5b505050506040513d601f19601f8201168201806040525081019061163491906123e3565b60ff16146116c4576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602e60248201527f53797374656d436f6e6669673a2062616420646563696d616c73206f6620676160448201527f7320706179696e6720746f6b656e00000000000000000000000000000000000060648201526084016108e4565b600061175f8273ffffffffffffffffffffffffffffffffffffffff166306fdde036040518163ffffffff1660e01b8152600401600060405180830381865afa158015611714573d6000803e3d6000fd5b505050506040513d6000823e601f3d9081017fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe016820160405261175a9190810190612400565b610f64565b905060006117b18373ffffffffffffffffffffffffffffffffffffffff166395d89b416040518163ffffffff1660e01b8152600401600060405180830381865afa158015611714573d6000803e3d6000fd5b90506117c08360128484611007565b6117c86106e0565b6040517f71cfaa3f00000000000000000000000000000000000000000000000000000000815273ffffffffffffffffffffffffffffffffffffffff858116600483015260126024830152604482018590526064820184905291909116906371cfaa3f90608401600060405180830381600087803b15801561184857600080fd5b505af115801561185c573d6000803e3d6000fd5b50505050505050565b8060a001516fffffffffffffffffffffffffffffffff16816060015163ffffffff161115611915576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152603560248201527f53797374656d436f6e6669673a206d696e206261736520666565206d7573742060448201527f6265206c657373207468616e206d61782062617365000000000000000000000060648201526084016108e4565b6001816040015160ff16116119ac576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602f60248201527f53797374656d436f6e6669673a2064656e6f6d696e61746f72206d757374206260448201527f65206c6172676572207468616e2031000000000000000000000000000000000060648201526084016108e4565b6068546080820151825167ffffffffffffffff909216916119cd91906124cb565b63ffffffff161115611a3b576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152601f60248201527f53797374656d436f6e6669673a20676173206c696d697420746f6f206c6f770060448201526064016108e4565b6000816020015160ff1611611ad2576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602f60248201527f53797374656d436f6e6669673a20656c6173746963697479206d756c7469706c60448201527f6965722063616e6e6f742062652030000000000000000000000000000000000060648201526084016108e4565b8051602082015163ffffffff82169160ff90911690611af29082906124ea565b611afc9190612534565b63ffffffff1614611b8f576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152603760248201527f53797374656d436f6e6669673a20707265636973696f6e206c6f73732077697460448201527f6820746172676574207265736f75726365206c696d697400000000000000000060648201526084016108e4565b805160698054602084015160408501516060860151608087015160a09097015163ffffffff9687167fffffffffffffffffffffffffffffffffffffffffffffffffffffff00000000009095169490941764010000000060ff94851602177fffffffffffffffffffffffffffffffffffffffffffff0000000000ffffffffff166501000000000093909216929092027fffffffffffffffffffffffffffffffffffffffffffff00000000ffffffffffff1617660100000000000091851691909102177fffff0000000000000000000000000000000000000000ffffffffffffffffffff166a010000000000000000000093909416929092027fffff00000000000000000000000000000000ffffffffffffffffffffffffffff16929092176e0100000000000000000000000000006fffffffffffffffffffffffffffffffff90921691909102179055565b60606000611ce5611102565b5090507fffffffffffffffffffffffff111111111111111111111111111111111111111273ffffffffffffffffffffffffffffffffffffffff821601611d5e57505060408051808201909152600381527f4554480000000000000000000000000000000000000000000000000000000000602082015290565b611d94611d8f6106d760017fa48b38a4b44951360fbdcbfaaeae5ed6ae92585412e9841b70ec72ed8cd057646123a0565b611f67565b91505090565b6033805473ffffffffffffffffffffffffffffffffffffffff8381167fffffffffffffffffffffffff0000000000000000000000000000000000000000831681179093556040519116919082907f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e090600090a35050565b60606000611e1d611102565b5090507fffffffffffffffffffffffff111111111111111111111111111111111111111273ffffffffffffffffffffffffffffffffffffffff821601611e9657505060408051808201909152600581527f4574686572000000000000000000000000000000000000000000000000000000602082015290565b611d94611d8f6106d760017f657c3582c29b3176614e3a33ddd1ec48352696a04e92b3c0566d72010fa8863d6123a0565b600054610100900460ff16611f5e576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152602b60248201527f496e697469616c697a61626c653a20636f6e7472616374206973206e6f74206960448201527f6e697469616c697a696e6700000000000000000000000000000000000000000060648201526084016108e4565b610ccf33611d9a565b60405160005b82811a15611f7d57600101611f6d565b80825260208201838152600082820152505060408101604052919050565b803573ffffffffffffffffffffffffffffffffffffffff81168114611fbf57600080fd5b919050565b600060208284031215611fd657600080fd5b611fdf82611f9b565b9392505050565b803567ffffffffffffffff81168114611fbf57600080fd5b7f4e487b7100000000000000000000000000000000000000000000000000000000600052604160045260246000fd5b60405160e0810167ffffffffffffffff8111828210171561205057612050611ffe565b60405290565b803563ffffffff81168114611fbf57600080fd5b60ff8116811461072157600080fd5b600060c0828403121561208b57600080fd5b60405160c0810181811067ffffffffffffffff821117156120ae576120ae611ffe565b6040529050806120bd83612056565b815260208301356120cd8161206a565b602082015260408301356120e08161206a565b60408201526120f160608401612056565b606082015261210260808401612056565b608082015260a08301356fffffffffffffffffffffffffffffffff8116811461212a57600080fd5b60a0919091015292915050565b6000806000806000806000806000898b0361028081121561215757600080fd5b6121608b611f9b565b995060208b0135985060408b0135975060608b0135965061218360808c01611fe6565b955061219160a08c01611f9b565b94506121a08c60c08d01612079565b93506121af6101808c01611f9b565b925060e07ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe60820112156121e157600080fd5b506121ea61202d565b6121f76101a08c01611f9b565b81526122066101c08c01611f9b565b60208201526122186101e08c01611f9b565b604082015261222a6102008c01611f9b565b606082015261223c6102208c01611f9b565b608082015261224e6102408c01611f9b565b60a08201526122606102608c01611f9b565b60c0820152809150509295985092959850929598565b60005b83811015612291578181015183820152602001612279565b838111156110d35750506000910152565b600081518084526122ba816020860160208601612276565b601f017fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe0169290920160200192915050565b602081526000611fdf60208301846122a2565b6000806040838503121561231257600080fd5b50508035926020909101359150565b60006020828403121561233357600080fd5b611fdf82611fe6565b600060c0828403121561234e57600080fd5b611fdf8383612079565b60006020828403121561236a57600080fd5b5035919050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601160045260246000fd5b6000828210156123b2576123b2612371565b500390565b600067ffffffffffffffff8083168185168083038211156123da576123da612371565b01949350505050565b6000602082840312156123f557600080fd5b8151611fdf8161206a565b60006020828403121561241257600080fd5b815167ffffffffffffffff8082111561242a57600080fd5b818401915084601f83011261243e57600080fd5b81518181111561245057612450611ffe565b604051601f82017fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe0908116603f0116810190838211818310171561249657612496611ffe565b816040528281528760208487010111156124af57600080fd5b6124c0836020830160208801612276565b979650505050505050565b600063ffffffff8083168185168083038211156123da576123da612371565b600063ffffffff80841680612528577f4e487b7100000000000000000000000000000000000000000000000000000000600052601260045260246000fd5b92169190910492915050565b600063ffffffff8083168185168183048111821515161561255757612557612371565b0294935050505056fea164736f6c634300080f000a"


func init() {
	if err := json.Unmarshal([]byte(SystemConfigStorageLayoutJSON), SystemConfigStorageLayout); err != nil {
		panic(err)
	}

	layouts["SystemConfig"] = SystemConfigStorageLayout
	deployedBytecodes["SystemConfig"] = SystemConfigDeployedBin
	immutableReferences["SystemConfig"] = false
}
