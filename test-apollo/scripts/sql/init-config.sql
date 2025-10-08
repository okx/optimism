USE `ApolloPortalDB`;

INSERT INTO `App` (`AppId`, `Name`, `OrgId`, `OrgName`, `OwnerName`, `OwnerEmail`)
VALUES ('XLayerSeq', 'XLayerSeq', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('XLayerRpc', 'XLayerRpc', 'default', 'Default', 'apollo', 'apollo@example.com');

INSERT INTO `AppNamespace` (`Name`, `AppId`, `Format`, `IsPublic`, `Comment`)
VALUES ('opgeth_jsonrpc-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth jsonrpc config for seq'),
       ('opnode_sequencer-config.txt', 'XLayerSeq', 'txt', 0, 'opnode sequencer config for seq'),
       ('opgeth_l2gaspricer-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth l2 gas pricer config for seq'),
       ('opgeth_pool-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth pool config for seq'),
       ('opgeth_jsonrpc-rpc-config.txt', 'XLayerRpc', 'txt', 0, 'opgeth jsonrpc config for rpc');

INSERT INTO `Permission` (`Id`, `PermissionType`, `TargetId`)
VALUES (1, 'CreateCluster', 'XLayerSeq'),
       (2, 'CreateNamespace', 'XLayerSeq'),
       (3, 'AssignRole', 'XLayerSeq'),
       (4, 'ModifyNamespace', 'XLayerSeq+opgeth_jsonrpc-config.txt'),
       (5, 'ReleaseNamespace', 'XLayerSeq+opgeth_jsonrpc-config.txt'),
       (6, 'ModifyNamespace', 'XLayerSeq+opnode_sequencer-config.txt'),
       (7, 'ReleaseNamespace', 'XLayerSeq+opnode_sequencer-config.txt'),
       (8, 'ModifyNamespace', 'XLayerSeq+opgeth_l2gaspricer-config.txt'),
       (9, 'ReleaseNamespace', 'XLayerSeq+opgeth_l2gaspricer-config.txt'),
       (10, 'ModifyNamespace', 'XLayerSeq+opgeth_pool-config.txt'),
       (11, 'ReleaseNamespace', 'XLayerSeq+opgeth_pool-config.txt'),
       (12, 'CreateCluster', 'XLayerRpc'),
       (13, 'CreateNamespace', 'XLayerRpc'),
       (14, 'AssignRole', 'XLayerRpc'),
       (15, 'ModifyNamespace', 'XLayerRpc+opgeth_jsonrpc-rpc-config.txt'),
       (16, 'ReleaseNamespace', 'XLayerRpc+opgeth_jsonrpc-rpc-config.txt');

INSERT INTO `Role` (`Id`, `RoleName`)
VALUES (1, 'Master+XLayerSeq'),
       (2, 'ModifyNamespace+XLayerSeq+opgeth_jsonrpc-config.txt'),
       (3, 'ReleaseNamespace+XLayerSeq+opgeth_jsonrpc-config.txt'),
       (4, 'ModifyNamespace+XLayerSeq+opnode_sequencer-config.txt'),
       (5, 'ReleaseNamespace+XLayerSeq+opnode_sequencer-config.txt'),
       (6, 'ModifyNamespace+XLayerSeq+opgeth_l2gaspricer-config.txt'),
       (7, 'ReleaseNamespace+XLayerSeq+opgeth_l2gaspricer-config.txt'),
       (8, 'ModifyNamespace+XLayerSeq+opgeth_pool-config.txt'),
       (9, 'ReleaseNamespace+XLayerSeq+opgeth_pool-config.txt'),
       (10, 'Master+XLayerRpc'),
       (11, 'ModifyNamespace+XLayerRpc+opgeth_jsonrpc-rpc-config.txt'),
       (12, 'ReleaseNamespace+XLayerRpc+opgeth_jsonrpc-rpc-config.txt');

INSERT INTO `RolePermission` (`RoleId`, `PermissionId`)
VALUES (1, 1),
       (1, 2),
       (1, 3),
       (2, 4),
       (3, 5),
       (4, 6),
       (5, 7),
       (6, 8),
       (7, 9),
       (8, 10),
       (9, 11),
       (10, 12),
       (10, 13),
       (10, 14),
       (11, 15),
       (12, 16);

INSERT INTO `UserRole` (`UserId`, `RoleId`)
VALUES ('apollo', 1),
       ('apollo', 2),
       ('apollo', 3),
       ('apollo', 4),
       ('apollo', 5),
       ('apollo', 6),
       ('apollo', 7),
       ('apollo', 8),
       ('apollo', 9),
       ('apollo', 10),
       ('apollo', 11),
       ('apollo', 12);

USE `ApolloConfigDB`;

INSERT INTO `App` (`AppId`, `Name`, `OrgId`, `OrgName`, `OwnerName`, `OwnerEmail`)
VALUES ('XLayerSeq', 'XLayerSeq', 'default', 'default', 'apollo', 'apollo@example.com'),
       ('XLayerRpc', 'XLayerRpc', 'default', 'Default', 'apollo', 'apollo@example.com');

INSERT INTO `Cluster` (`Name`, `AppId`, `IsDeleted`)
VALUES ('default', 'XLayerSeq', 0),
       ('default', 'XLayerRpc', 0);

INSERT INTO `AppNamespace` (`Name`, `AppId`, `Format`, `IsPublic`, `Comment`)
VALUES ('opgeth_jsonrpc-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth jsonrpc config for seq'),
       ('opnode_sequencer-config.txt', 'XLayerSeq', 'txt', 0, 'opnode sequencer config for seq'),
       ('opgeth_l2gaspricer-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth l2 gas pricer config for seq'),
       ('opgeth_pool-config.txt', 'XLayerSeq', 'txt', 0, 'opgeth pool config for seq'),
       ('opgeth_jsonrpc-rpc-config.txt', 'XLayerRpc', 'txt', 0, 'opgeth jsonrpc config for rpc');

INSERT INTO `Namespace` (`AppId`, `ClusterName`, `NamespaceName`, `IsDeleted`)
VALUES ('XLayerSeq', 'default', 'opgeth_jsonrpc-config.txt', 0),
       ('XLayerSeq', 'default', 'opnode_sequencer-config.txt', 0),
       ('XLayerSeq', 'default', 'opgeth_l2gaspricer-config.txt', 0),
       ('XLayerSeq', 'default', 'opgeth_pool-config.txt', 0),
       ('XLayerRpc', 'default', 'opgeth_jsonrpc-rpc-config.txt', 0);

INSERT INTO `Item` (`NamespaceId`, `Key`, `Type`, `Value`, `IsDeleted`)
VALUES ('2', 'content', 0, '', 0),
       ('3', 'content', 0, 'gpo.factor: 0\ngpo.default: 100000000\ngpo.maxprice: 500000000000\n', 0),
       ('4', 'content', 0, '', 0),
       ('5', 'content', 0, '', 0);

