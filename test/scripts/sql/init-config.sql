USE `ApolloPortalDB`;

INSERT INTO `App` (`AppId`, `Name`, `OrgId`, `OrgName`, `OwnerName`, `OwnerEmail`)
VALUES ('OpSeq', 'OpSeq', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpRpc', 'OpRpc', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpBatcher', 'OpBatcher', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpProposer', 'OpProposer', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpChallenger', 'OpChallenger', 'default', 'Default', 'apollo', 'apollo@example.com');

INSERT INTO `AppNamespace` (`Name`, `AppId`, `Format`, `IsPublic`, `Comment`)
VALUES ('op-seq.txt', 'OpSeq', 'txt', 0, 'Sequencer node config'),
       ('op-rpc.txt', 'OpRpc', 'txt', 0, 'RPC node config'),
       ('op-batcher.txt', 'OpBatcher', 'txt', 0, 'Op Batcher config'),
       ('op-proposer.txt', 'OpProposer', 'txt', 0, 'Op Proposer config'),
       ('op-challenger.txt', 'OpChallenger', 'txt', 0, 'Op Challenger config');

INSERT INTO `Permission` (`Id`, `PermissionType`, `TargetId`)
VALUES (1, 'CreateCluster', 'OpSeq'),
       (2, 'CreateNamespace', 'OpSeq'),
       (3, 'AssignRole', 'OpSeq'),
       (4, 'ModifyNamespace', 'OpSeq+op-seq.txt'),
       (5, 'ReleaseNamespace', 'OpSeq+op-seq.txt'),
       (6, 'CreateCluster', 'OpRpc'),
       (7, 'CreateNamespace', 'OpRpc'),
       (8, 'AssignRole', 'OpRpc'),
       (9, 'ModifyNamespace', 'OpRpc+op-rpc.txt'),
       (10, 'ReleaseNamespace', 'OpRpc+op-rpc.txt'),
       (11, 'CreateCluster', 'OpBatcher'),
       (12, 'CreateNamespace', 'OpBatcher'),
       (13, 'AssignRole', 'OpBatcher'),
       (14, 'ModifyNamespace', 'OpBatcher+op-batcher.txt'),
       (15, 'ReleaseNamespace', 'OpBatcher+op-batcher.txt'),
       (16, 'CreateCluster', 'OpProposer'),
       (17, 'CreateNamespace', 'OpProposer'),
       (18, 'AssignRole', 'OpProposer'),
       (19, 'ModifyNamespace', 'OpProposer+op-proposer.txt'),
       (20, 'ReleaseNamespace', 'OpProposer+op-proposer.txt'),
       (21, 'CreateCluster', 'OpChallenger'),
       (22, 'CreateNamespace', 'OpChallenger'),
       (23, 'AssignRole', 'OpChallenger'),
       (24, 'ModifyNamespace', 'OpChallenger+op-challenger.txt'),
       (25, 'ReleaseNamespace', 'OpChallenger+op-challenger.txt');

INSERT INTO `Role` (`Id`, `RoleName`)
VALUES (1, 'Master+OpSeq'),
       (2, 'ModifyNamespace+OpSeq+op-seq.txt'),
       (3, 'ReleaseNamespace+OpSeq+op-seq.txt'),
       (4, 'Master+OpRpc'),
       (5, 'ModifyNamespace+OpRpc+op-rpc.txt'),
       (6, 'ReleaseNamespace+OpRpc+op-rpc.txt'),
       (7, 'Master+OpBatcher'),
       (8, 'ModifyNamespace+OpBatcher+op-batcher.txt'),
       (9, 'ReleaseNamespace+OpBatcher+op-batcher.txt'),
       (10, 'Master+OpProposer'),
       (11, 'ModifyNamespace+OpProposer+op-proposer.txt'),
       (12, 'ReleaseNamespace+OpProposer+op-proposer.txt'),
       (13, 'Master+OpChallenger'),
       (14, 'ModifyNamespace+OpChallenger+op-challenger.txt'),
       (15, 'ReleaseNamespace+OpChallenger+op-challenger.txt');

INSERT INTO `RolePermission` (`RoleId`, `PermissionId`)
VALUES (1, 1),
       (1, 2),
       (1, 3),
       (2, 4),
       (3, 5),
       (4, 6),
       (4, 7),
       (4, 8),
       (5, 9),
       (6, 10),
       (7, 11),
       (7, 12),
       (7, 13),
       (8, 14),
       (9, 15),
       (10, 16),
       (10, 17),
       (10, 18),
       (11, 19),
       (12, 20),
       (13, 21),
       (13, 22),
       (13, 23),
       (14, 24),
       (15, 25);

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
       ('apollo', 12),
       ('apollo', 13),
       ('apollo', 14),
       ('apollo', 15);

USE `ApolloConfigDB`;

INSERT INTO `App` (`AppId`, `Name`, `OrgId`, `OrgName`, `OwnerName`, `OwnerEmail`)
VALUES ('OpSeq', 'OpSeq', 'default', 'default', 'apollo', 'apollo@example.com'),
       ('OpRpc', 'OpRpc', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpBatcher', 'OpBatcher', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpProposer', 'OpProposer', 'default', 'Default', 'apollo', 'apollo@example.com'),
       ('OpChallenger', 'OpChallenger', 'default', 'Default', 'apollo', 'apollo@example.com');

INSERT INTO `Cluster` (`Name`, `AppId`, `IsDeleted`)
VALUES ('default', 'OpSeq', 0),
       ('default', 'OpRpc', 0),
       ('default', 'OpBatcher', 0),
       ('default', 'OpProposer', 0),
       ('default', 'OpChallenger', 0);

INSERT INTO `AppNamespace` (`Name`, `AppId`, `Format`, `IsPublic`, `Comment`)
VALUES ('op-seq.txt', 'OpSeq', 'txt', 0, 'Sequencer node config'),
       ('op-rpc.txt', 'OpRpc', 'txt', 0, 'RPC node config'),
       ('op-batcher.txt', 'OpBatcher', 'txt', 0, 'Op Batcher config'),
       ('op-proposer.txt', 'OpProposer', 'txt', 0, 'Op Proposer config'),
       ('op-challenger.txt', 'OpChallenger', 'txt', 0, 'Op Challenger config');

INSERT INTO `Namespace` (`AppId`, `ClusterName`, `NamespaceName`, `IsDeleted`)
VALUES ('OpSeq', 'default', 'op-seq.txt', 0),
       ('OpRpc', 'default', 'op-rpc.txt', 0),
       ('OpBatcher', 'default', 'op-batcher.txt', 0),
       ('OpProposer', 'default', 'op-proposer.txt', 0),
       ('OpChallenger', 'default', 'op-challenger.txt', 0);

INSERT INTO `Item` (`NamespaceId`, `Key`, `Type`, `Value`, `IsDeleted`)
VALUES ('1', 'content', 0, 'l1.epoch-poll-interval: 12s', 0),
       ('3', 'content', 0, 'max-channel-duration: 40', 0);

