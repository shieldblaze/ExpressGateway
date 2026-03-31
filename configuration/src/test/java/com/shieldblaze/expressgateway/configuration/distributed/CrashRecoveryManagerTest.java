/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.configuration.distributed;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyInt;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

class CrashRecoveryManagerTest {

    private ConfigVersionStore versionStore;
    private ConfigQuorumManager quorumManager;
    private ConfigAuditLog auditLog;
    private ConfigFallbackStore fallbackStore;

    @BeforeEach
    void setUp() {
        // Disable stabilization delay for tests
        System.setProperty("expressgateway.recovery.stabilization.ms", "0");

        versionStore = mock(ConfigVersionStore.class);
        quorumManager = mock(ConfigQuorumManager.class);
        auditLog = mock(ConfigAuditLog.class);
        fallbackStore = mock(ConfigFallbackStore.class);
    }

    @AfterEach
    void tearDown() {
        System.clearProperty("expressgateway.recovery.stabilization.ms");
    }

    @Test
    void testNoOrphanedProposals() throws Exception {
        // Current version == highest version -> no orphans
        when(versionStore.currentVersion()).thenReturn(3);
        when(versionStore.listVersions()).thenReturn(List.of(1, 2, 3));

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        // Should not attempt any commit or rollback
        verify(versionStore, never()).setCurrent(anyInt());
        verify(quorumManager, never()).cleanupAcks(anyString());
    }

    @Test
    void testNoVersionsExist() throws Exception {
        when(versionStore.currentVersion()).thenReturn(-1);
        when(versionStore.listVersions()).thenReturn(List.of());

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        verify(versionStore, never()).setCurrent(anyInt());
    }

    @Test
    void testOrphanedVersionWithQuorum_CommitsIt() throws Exception {
        // Orphaned version 5 exists beyond current version 4
        when(versionStore.currentVersion()).thenReturn(4);
        when(versionStore.listVersions()).thenReturn(List.of(1, 2, 3, 4, 5));

        // Quorum: 3-node cluster needs 2 ACKs. Orphan has 2 -> quorum met.
        when(quorumManager.getAckCount("v005")).thenReturn(2);

        // readVersion for LKG update
        when(versionStore.readVersion(5)).thenReturn(ConfigurationContext.DEFAULT);

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        // Should commit the orphaned version
        verify(versionStore).setCurrent(5);

        // Should update LKG
        verify(fallbackStore).saveLastKnownGood(any(ConfigurationContext.class));

        // Should log recovery
        verify(auditLog).log(eq("v005"), eq(ConfigAuditLog.AuditAction.RECOVERY), anyString(), anyString());
    }

    @Test
    void testOrphanedVersionWithoutQuorum_AbandonsIt() throws Exception {
        // Orphaned version 5 exists beyond current version 4
        when(versionStore.currentVersion()).thenReturn(4);
        when(versionStore.listVersions()).thenReturn(List.of(1, 2, 3, 4, 5));

        // Quorum: 3-node cluster needs 2 ACKs. Orphan has only 1 -> no quorum.
        when(quorumManager.getAckCount("v005")).thenReturn(1);

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        // Should NOT commit the orphaned version
        verify(versionStore, never()).setCurrent(anyInt());

        // Should clean up ACKs for the abandoned version
        verify(quorumManager).cleanupAcks("v005");

        // Should log recovery
        verify(auditLog).log(eq("v005"), eq(ConfigAuditLog.AuditAction.RECOVERY), anyString(), anyString());
    }

    @Test
    void testInitialProposalCrash_NoCurrentPointer() throws Exception {
        // No current pointer (-1), but version 1 exists
        when(versionStore.currentVersion()).thenReturn(-1);
        when(versionStore.listVersions()).thenReturn(List.of(1));

        // With 3 nodes, quorum = 2. Version 1 has 2 ACKs.
        when(quorumManager.getAckCount("v001")).thenReturn(2);
        when(versionStore.readVersion(1)).thenReturn(ConfigurationContext.DEFAULT);

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        // Should commit the initial version
        verify(versionStore).setCurrent(1);
    }

    @Test
    void testMultipleOrphanedVersions_OnlyRecoversMostRecent() throws Exception {
        // Versions 4 and 5 are both beyond current 3
        when(versionStore.currentVersion()).thenReturn(3);
        when(versionStore.listVersions()).thenReturn(List.of(1, 2, 3, 4, 5));

        // Only the highest orphan (5) is checked
        when(quorumManager.getAckCount("v005")).thenReturn(0);

        CrashRecoveryManager manager = new CrashRecoveryManager(
                versionStore, quorumManager, auditLog, fallbackStore, 3);

        manager.recoverIfNeeded();

        // Should only check v005, not v004
        verify(quorumManager).getAckCount("v005");
        verify(quorumManager, never()).getAckCount("v004");

        // v005 abandoned (0 ACKs < 2 required)
        verify(quorumManager).cleanupAcks("v005");
    }

    @Test
    void testQuorumSizeCalculation() {
        // Quorum is strict majority: (N/2) + 1
        org.junit.jupiter.api.Assertions.assertEquals(1, ConfigQuorumManager.quorumSize(1));
        org.junit.jupiter.api.Assertions.assertEquals(2, ConfigQuorumManager.quorumSize(2));
        org.junit.jupiter.api.Assertions.assertEquals(2, ConfigQuorumManager.quorumSize(3));
        org.junit.jupiter.api.Assertions.assertEquals(3, ConfigQuorumManager.quorumSize(4));
        org.junit.jupiter.api.Assertions.assertEquals(3, ConfigQuorumManager.quorumSize(5));
    }

    @Test
    void testVersionFormatting() {
        org.junit.jupiter.api.Assertions.assertEquals("v001", ConfigQuorumManager.formatVersion(1));
        org.junit.jupiter.api.Assertions.assertEquals("v005", ConfigQuorumManager.formatVersion(5));
        org.junit.jupiter.api.Assertions.assertEquals("v100", ConfigQuorumManager.formatVersion(100));
    }

    @Test
    void testInvalidConstructorArgs() {
        org.junit.jupiter.api.Assertions.assertThrows(NullPointerException.class, () ->
                new CrashRecoveryManager(null, quorumManager, auditLog, fallbackStore, 3));
        org.junit.jupiter.api.Assertions.assertThrows(NullPointerException.class, () ->
                new CrashRecoveryManager(versionStore, null, auditLog, fallbackStore, 3));
        org.junit.jupiter.api.Assertions.assertThrows(IllegalArgumentException.class, () ->
                new CrashRecoveryManager(versionStore, quorumManager, auditLog, fallbackStore, 0));
    }
}
