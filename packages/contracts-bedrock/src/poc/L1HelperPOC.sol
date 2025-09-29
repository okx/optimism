// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

interface IERC20 {
    function transferFrom(
        address from,
        address to,
        uint256 amount
    ) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

interface IOptimismPortal2 {
    function depositTransaction(
        address _to,
        uint256 _value,
        uint64 _gasLimit,
        bool _isCreation,
        bytes memory _data
    ) external payable;
}

contract L1HelperPOC {
    address OptimismPortal2;
    address OKB;
    constructor(address _OptimismPortal2, address _OKB) {
        OptimismPortal2 = _OptimismPortal2;
        OKB = _OKB;
    }

    function bridgeOKB(uint256 _amount) external {
        require(
            IERC20(OKB).balanceOf(msg.sender) >= _amount,
            "Insufficient balance"
        );
        //in production, we should call triggerBridge to burn
        IERC20(OKB).transferFrom(msg.sender, address(1), _amount);


        IOptimismPortal2(OptimismPortal2).depositTransaction(
            msg.sender,
            _amount,
            21000,
            false,
            ""
        );
    }
}
