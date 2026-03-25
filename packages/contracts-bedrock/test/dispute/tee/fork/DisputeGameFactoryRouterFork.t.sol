// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Vm } from "forge-std/Vm.sol";
import { Proxy } from "src/universal/Proxy.sol";
import { AnchorStateRegistry } from "src/dispute/AnchorStateRegistry.sol";
import { DisputeGameFactory } from "src/dispute/DisputeGameFactory.sol";
import { PermissionedDisputeGame } from "src/dispute/PermissionedDisputeGame.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";
import { DisputeGameFactoryRouter } from "src/dispute/DisputeGameFactoryRouter.sol";
import { IDisputeGameFactoryRouter } from "interfaces/dispute/IDisputeGameFactoryRouter.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { Claim, Duration, GameStatus, GameType, Hash, Proposal } from "src/dispute/lib/Types.sol";
import { TeeTestUtils } from "test/dispute/tee/helpers/TeeTestUtils.sol";
import { MockRiscZeroVerifier } from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import { MockSystemConfig } from "test/dispute/tee/mocks/MockSystemConfig.sol";

contract DisputeGameFactoryRouterForkTest is TeeTestUtils {
    struct SecondZoneFixture {
        DisputeGameFactory factory;
        AnchorStateRegistry anchorStateRegistry;
        TeeProofVerifier teeProofVerifier;
        TeeDisputeGame implementation;
        address registeredExecutor;
    }

    struct XLayerConfig {
        GameType gameType;
        uint256 initBond;
        address proposer;
    }

    uint256 internal constant DEFENDER_BOND = 1 ether;
    uint256 internal constant CHALLENGER_BOND = 2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 1 days;
    uint64 internal constant MAX_PROVE_DURATION = 12 hours;
    bytes32 internal constant IMAGE_ID = keccak256("fork-tee-image");
    bytes32 internal constant PCR_HASH = keccak256("fork-pcr-hash");

    // XLayer's dispute game factory is deployed on Ethereum mainnet L1.
    address internal constant XLAYER_FACTORY = 0x9D4c8FAEadDdDeeE1Ed0c92dAbAD815c2484f675;
    uint256 internal constant ZONE_XLAYER = 1;
    uint256 internal constant ZONE_SECOND = 2;
    GameType internal constant XLAYER_GAME_TYPE = GameType.wrap(1);
    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    bytes32 internal constant ANCHOR_BLOCK_HASH = keccak256("fork-anchor-block");
    bytes32 internal constant ANCHOR_STATE_HASH = keccak256("fork-anchor-state");
    uint256 internal constant ANCHOR_L2_BLOCK = 10;

    DisputeGameFactory internal xLayerFactory;
    DisputeGameFactoryRouter internal router;
    bool internal hasFork;

    address internal proposer;
    address internal challenger;

    function setUp() public {
        proposer = makeWallet(DEFAULT_PROPOSER_KEY, "fork-proposer").addr;
        challenger = makeWallet(DEFAULT_CHALLENGER_KEY, "fork-challenger").addr;

        if (!vm.envExists("ETH_RPC_URL")) return;

        vm.createSelectFork(vm.envString("ETH_RPC_URL"));
        hasFork = true;
        xLayerFactory = DisputeGameFactory(XLAYER_FACTORY);
        router = new DisputeGameFactoryRouter(address(this));
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    function test_liveFactoryReadPaths() public view {
        if (!hasFork) return;

        _assertLiveFactoryFork();

        assertTrue(xLayerFactory.owner() != address(0));
        assertTrue(bytes(xLayerFactory.version()).length > 0);
        assertTrue(address(xLayerFactory.gameImpls(XLAYER_GAME_TYPE)) != address(0));
        assertGt(xLayerFactory.initBonds(XLAYER_GAME_TYPE), 0);
    }

    function test_routerCreate_onlyXLayer() public {
        if (!hasFork) return;

        _assertLiveFactoryFork();
        XLayerConfig memory xLayer = _readXLayerConfig();

        vm.deal(xLayer.proposer, 10 ether);

        Claim rootClaim = Claim.wrap(keccak256("xlayer-router-root"));
        bytes memory extraData = abi.encodePacked(uint256(1_000_000_000));

        vm.startPrank(xLayer.proposer, xLayer.proposer);
        address proxy = router.create{ value: xLayer.initBond }(ZONE_XLAYER, xLayer.gameType, rootClaim, extraData);
        vm.stopPrank();

        assertTrue(proxy != address(0));

        (IDisputeGame storedGame,) = xLayerFactory.games(xLayer.gameType, rootClaim, extraData);
        assertEq(address(storedGame), proxy);
        assertEq(storedGame.gameCreator(), address(router));
        assertEq(storedGame.rootClaim().raw(), rootClaim.raw());
    }

    function test_routerCreate_onlySecondZone_lifecycle() public {
        if (!hasFork) return;

        _assertLiveFactoryFork();
        SecondZoneFixture memory secondZone = _installSecondZoneFixture(proposer);

        vm.deal(proposer, 100 ether);
        vm.deal(challenger, 100 ether);

        bytes32 endBlockHash = keccak256("second-zone-end-block");
        bytes32 endStateHash = keccak256("second-zone-end-state");
        (TeeDisputeGame game, Claim rootClaim, bytes memory extraData) =
            _createSecondZoneGame(endBlockHash, endStateHash, ANCHOR_L2_BLOCK + 6);

        _assertStoredSecondZoneGame(secondZone.factory, game, rootClaim, extraData);
        assertEq(game.gameCreator(), address(router));
        assertEq(game.proposer(), proposer);

        _runSecondZoneLifecycle(secondZone, game, endBlockHash, endStateHash);
    }

    function test_routerCreateBatch_xLayerAndSecondZone() public {
        if (!hasFork) return;

        _assertLiveFactoryFork();
        XLayerConfig memory xLayer = _readXLayerConfig();
        SecondZoneFixture memory secondZone = _installSecondZoneFixture(xLayer.proposer);

        vm.deal(xLayer.proposer, 10 ether);
        vm.deal(proposer, 100 ether);
        vm.deal(challenger, 100 ether);

        Claim xLayerRootClaim = Claim.wrap(keccak256("batch-xlayer-root"));
        bytes memory xLayerExtraData = abi.encodePacked(uint256(1_000_000_001));

        bytes32 secondZoneEndBlockHash = keccak256("batch-second-zone-end-block");
        bytes32 secondZoneEndStateHash = keccak256("batch-second-zone-end-state");
        bytes memory secondZoneExtraData =
            buildExtraData(ANCHOR_L2_BLOCK + 8, type(uint32).max, secondZoneEndBlockHash, secondZoneEndStateHash);
        Claim secondZoneRootClaim = computeRootClaim(secondZoneEndBlockHash, secondZoneEndStateHash);

        IDisputeGameFactoryRouter.CreateParams[] memory params = new IDisputeGameFactoryRouter.CreateParams[](2);
        params[0] = IDisputeGameFactoryRouter.CreateParams({
            zoneId: ZONE_XLAYER,
            gameType: xLayer.gameType,
            rootClaim: xLayerRootClaim,
            extraData: xLayerExtraData,
            bond: xLayer.initBond
        });
        params[1] = IDisputeGameFactoryRouter.CreateParams({
            zoneId: ZONE_SECOND,
            gameType: TEE_GAME_TYPE,
            rootClaim: secondZoneRootClaim,
            extraData: secondZoneExtraData,
            bond: DEFENDER_BOND
        });

        vm.startPrank(xLayer.proposer, xLayer.proposer);
        address[] memory proxies = router.createBatch{ value: xLayer.initBond + DEFENDER_BOND }(params);
        vm.stopPrank();

        assertEq(proxies.length, 2);

        (IDisputeGame xLayerStoredGame,) = xLayerFactory.games(xLayer.gameType, xLayerRootClaim, xLayerExtraData);
        assertEq(address(xLayerStoredGame), proxies[0]);
        assertEq(xLayerStoredGame.gameCreator(), address(router));

        TeeDisputeGame secondZoneGame = TeeDisputeGame(payable(proxies[1]));
        _assertStoredSecondZoneGame(secondZone.factory, secondZoneGame, secondZoneRootClaim, secondZoneExtraData);
        assertEq(secondZoneGame.gameCreator(), address(router));
        assertEq(secondZoneGame.proposer(), xLayer.proposer);

        _runSecondZoneLifecycle(secondZone, secondZoneGame, secondZoneEndBlockHash, secondZoneEndStateHash);
    }

    function _readXLayerConfig() internal view returns (XLayerConfig memory xLayer) {
        xLayer.gameType = XLAYER_GAME_TYPE;
        xLayer.initBond = xLayerFactory.initBonds(XLAYER_GAME_TYPE);

        PermissionedDisputeGame implementation =
            PermissionedDisputeGame(payable(address(xLayerFactory.gameImpls(XLAYER_GAME_TYPE))));
        xLayer.proposer = implementation.proposer();

        assertTrue(address(implementation) != address(0), "xlayer impl missing");
        assertGt(xLayer.initBond, 0, "xlayer init bond missing");
    }

    function _installSecondZoneFixture(address allowedProposer)
        internal
        returns (SecondZoneFixture memory secondZone)
    {
        secondZone.factory = _deployLocalDisputeGameFactory();
        router.setZone(ZONE_SECOND, address(secondZone.factory));

        secondZone.anchorStateRegistry = _deployRealAnchorStateRegistry(secondZone.factory);
        (secondZone.teeProofVerifier, secondZone.registeredExecutor) = _deployRealTeeProofVerifier();

        secondZone.implementation = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(secondZone.factory)),
            ITeeProofVerifier(address(secondZone.teeProofVerifier)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(secondZone.anchorStateRegistry)),
            allowedProposer,
            challenger
        );

        secondZone.factory.setImplementation(TEE_GAME_TYPE, IDisputeGame(address(secondZone.implementation)), bytes(""));
        secondZone.factory.setInitBond(TEE_GAME_TYPE, DEFENDER_BOND);

        // Real ASR marks games created at or before the retirement timestamp as retired.
        vm.warp(block.timestamp + 1);
    }

    function _createSecondZoneGame(
        bytes32 endBlockHash,
        bytes32 endStateHash,
        uint256 l2SequenceNumber
    )
        internal
        returns (TeeDisputeGame game, Claim rootClaim, bytes memory extraData)
    {
        extraData = buildExtraData(l2SequenceNumber, type(uint32).max, endBlockHash, endStateHash);
        rootClaim = computeRootClaim(endBlockHash, endStateHash);

        vm.startPrank(proposer, proposer);
        address proxy = router.create{ value: DEFENDER_BOND }(ZONE_SECOND, TEE_GAME_TYPE, rootClaim, extraData);
        vm.stopPrank();

        game = TeeDisputeGame(payable(proxy));
    }

    function _runSecondZoneLifecycle(
        SecondZoneFixture memory secondZone,
        TeeDisputeGame game,
        bytes32 endBlockHash,
        bytes32 endStateHash
    )
        internal
    {
        (Hash startingRoot, uint256 startingBlockNumber) = game.startingOutputRoot();
        assertEq(startingRoot.raw(), computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw());
        assertEq(startingBlockNumber, ANCHOR_L2_BLOCK);

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        address gameProposer = game.proposer();

        vm.prank(gameProposer);
        game.prove(abi.encode(proofs));

        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));
        assertTrue(secondZone.anchorStateRegistry.isGameRegistered(game));
        assertTrue(secondZone.anchorStateRegistry.isGameProper(game));
        assertFalse(secondZone.anchorStateRegistry.isGameFinalized(game));
        assertTrue(secondZone.teeProofVerifier.isRegistered(secondZone.registeredExecutor));

        vm.warp(block.timestamp + 1);
        assertTrue(secondZone.anchorStateRegistry.isGameFinalized(game));
        assertEq(game.normalModeCredit(gameProposer), DEFENDER_BOND + CHALLENGER_BOND);

        uint256 proposerBalanceBefore = gameProposer.balance;
        game.claimCredit(gameProposer);
        assertEq(gameProposer.balance, proposerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);
        assertEq(address(secondZone.anchorStateRegistry.anchorGame()), address(game));
    }

    function _assertStoredSecondZoneGame(
        DisputeGameFactory factory,
        TeeDisputeGame game,
        Claim rootClaim,
        bytes memory extraData
    )
        internal
        view
    {
        (IDisputeGame storedGame,) = factory.games(TEE_GAME_TYPE, rootClaim, extraData);
        assertEq(address(storedGame), address(game));
        assertEq(game.rootClaim().raw(), rootClaim.raw());
    }

    function _deployLocalDisputeGameFactory() internal returns (DisputeGameFactory factory) {
        DisputeGameFactory implementation = new DisputeGameFactory();
        Proxy proxy = new Proxy(address(this));
        proxy.upgradeToAndCall(address(implementation), abi.encodeCall(implementation.initialize, (address(this))));
        factory = DisputeGameFactory(address(proxy));
    }

    function _deployRealAnchorStateRegistry(DisputeGameFactory factory)
        internal
        returns (AnchorStateRegistry anchorStateRegistry)
    {
        MockSystemConfig systemConfig = new MockSystemConfig(address(this));
        AnchorStateRegistry anchorStateRegistryImpl = new AnchorStateRegistry(0);
        Proxy anchorStateRegistryProxy = new Proxy(address(this));
        anchorStateRegistryProxy.upgradeToAndCall(
            address(anchorStateRegistryImpl),
            abi.encodeCall(
                anchorStateRegistryImpl.initialize,
                (
                    ISystemConfig(address(systemConfig)),
                    IDisputeGameFactory(address(factory)),
                    Proposal({
                        root: Hash.wrap(computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw()),
                        l2SequenceNumber: ANCHOR_L2_BLOCK
                    }),
                    TEE_GAME_TYPE
                )
            )
        );
        anchorStateRegistry = AnchorStateRegistry(address(anchorStateRegistryProxy));
    }

    function _deployRealTeeProofVerifier()
        internal
        returns (TeeProofVerifier teeProofVerifier, address registeredExecutor)
    {
        Vm.Wallet memory enclaveWallet = makeWallet(DEFAULT_EXECUTOR_KEY, "fork-registered-enclave");
        MockRiscZeroVerifier riscZeroVerifier = new MockRiscZeroVerifier();
        bytes memory expectedRootKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)), bytes32(uint256(3)));

        registeredExecutor = enclaveWallet.addr;
        teeProofVerifier = new TeeProofVerifier(riscZeroVerifier, IMAGE_ID, expectedRootKey);
        TeeProofVerifier.AttestationData memory data = TeeProofVerifier.AttestationData({
            timestampMs: 1234,
            pcrHash: PCR_HASH,
            publicKey: uncompressedPublicKey(enclaveWallet),
            userData: ""
        });
        teeProofVerifier.register("", data);
    }

    function _assertLiveFactoryFork() internal view {
        assertEq(block.chainid, 1, "expected Ethereum mainnet fork");
        assertTrue(XLAYER_FACTORY.code.length > 0, "factory missing on fork");
    }
}
