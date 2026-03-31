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
package com.shieldblaze.expressgateway.controlplane.resilience;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigCorruptionHandlerTest {

    private ConfigCorruptionHandler handler;
    private boolean rollbackCalled;
    private boolean rollbackReturn;

    @BeforeEach
    void setUp() {
        rollbackCalled = false;
        rollbackReturn = true;
        handler = new ConfigCorruptionHandler(path -> {
            rollbackCalled = true;
            return rollbackReturn;
        });
    }

    @Test
    void validDataPassesVerification() {
        byte[] data = "test config data".getBytes(StandardCharsets.UTF_8);
        String checksum = ConfigCorruptionHandler.computeChecksum(data);

        assertTrue(handler.verify("/config/cluster-1", data, checksum));
        assertFalse(rollbackCalled);
        assertEquals(0, handler.corruptionCount());
    }

    @Test
    void corruptDataTriggersRollback() {
        byte[] data = "test config data".getBytes(StandardCharsets.UTF_8);
        String wrongChecksum = "0000000000000000000000000000000000000000000000000000000000000000";

        assertFalse(handler.verify("/config/cluster-1", data, wrongChecksum));
        assertTrue(rollbackCalled);
        assertEquals(1, handler.corruptionCount());
    }

    @Test
    void corruptionEventRecorded() {
        byte[] data = "test data".getBytes(StandardCharsets.UTF_8);
        handler.verify("/config/test", data, "bad-checksum");

        List<ConfigCorruptionHandler.CorruptionEvent> events = handler.auditLog();
        assertEquals(1, events.size());

        ConfigCorruptionHandler.CorruptionEvent event = events.get(0);
        assertEquals("/config/test", event.resourcePath());
        assertEquals("bad-checksum", event.expectedChecksum());
        assertTrue(event.rollbackAttempted());
        assertTrue(event.rollbackSuccess());
    }

    @Test
    void rollbackFailureRecordedInEvent() {
        rollbackReturn = false;
        byte[] data = "test data".getBytes(StandardCharsets.UTF_8);
        handler.verify("/config/test", data, "bad-checksum");

        ConfigCorruptionHandler.CorruptionEvent event = handler.auditLog().get(0);
        assertTrue(event.rollbackAttempted());
        assertFalse(event.rollbackSuccess());
    }

    @Test
    void alertListenerFired() {
        List<ConfigCorruptionHandler.CorruptionEvent> alerts = new ArrayList<>();
        handler.addAlertListener(alerts::add);

        byte[] data = "test data".getBytes(StandardCharsets.UTF_8);
        handler.verify("/config/test", data, "bad-checksum");

        assertEquals(1, alerts.size());
    }

    @Test
    void computeChecksumIsDeterministic() {
        byte[] data = "deterministic data".getBytes(StandardCharsets.UTF_8);
        String c1 = ConfigCorruptionHandler.computeChecksum(data);
        String c2 = ConfigCorruptionHandler.computeChecksum(data);
        assertEquals(c1, c2);
    }

    @Test
    void computeChecksumIsHex64Chars() {
        String checksum = ConfigCorruptionHandler.computeChecksum("test".getBytes());
        assertEquals(64, checksum.length()); // SHA-256 = 32 bytes = 64 hex chars
        assertTrue(checksum.matches("[0-9a-f]+"));
    }

    @Test
    void auditLogTrimmedToMaxSize() {
        ConfigCorruptionHandler smallHandler = new ConfigCorruptionHandler(p -> true, 3);
        byte[] data = "data".getBytes();
        for (int i = 0; i < 10; i++) {
            smallHandler.verify("/config/" + i, data, "wrong");
        }
        assertEquals(3, smallHandler.auditLog().size());
    }

    @Test
    void rollbackExceptionDoesNotPropagateButIsRecorded() {
        ConfigCorruptionHandler throwingHandler = new ConfigCorruptionHandler(path -> {
            throw new RuntimeException("rollback error");
        });

        byte[] data = "data".getBytes();
        assertFalse(throwingHandler.verify("/test", data, "wrong"));

        ConfigCorruptionHandler.CorruptionEvent event = throwingHandler.auditLog().get(0);
        assertTrue(event.rollbackAttempted());
        assertFalse(event.rollbackSuccess());
    }
}
