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
package com.shieldblaze.expressgateway.controlplane.cluster;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Unit tests for {@link ControlPlaneInstance}.
 *
 * <p>Validates record construction, field validation, and the {@code withHeartbeat}
 * copy method that produces a new instance with an updated heartbeat timestamp.</p>
 */
class ControlPlaneInstanceTest {

    private static final String INSTANCE_ID = "cp-instance-1";
    private static final String REGION = "us-east-1";
    private static final String GRPC_ADDRESS = "10.0.0.1";
    private static final int GRPC_PORT = 9443;
    private static final long REGISTERED_AT = System.currentTimeMillis();
    private static final long LAST_HEARTBEAT = REGISTERED_AT + 1000;

    @Test
    void validConstructionWithAllFields() {
        ControlPlaneInstance instance = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT);

        assertEquals(INSTANCE_ID, instance.instanceId());
        assertEquals(REGION, instance.region());
        assertEquals(GRPC_ADDRESS, instance.grpcAddress());
        assertEquals(GRPC_PORT, instance.grpcPort());
        assertEquals(REGISTERED_AT, instance.registeredAt());
        assertEquals(LAST_HEARTBEAT, instance.lastHeartbeat());
    }

    @Test
    void validConstructionWithBoundaryPort() {
        // Minimum valid port
        ControlPlaneInstance minPort = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, 1, REGISTERED_AT, LAST_HEARTBEAT);
        assertEquals(1, minPort.grpcPort());

        // Maximum valid port
        ControlPlaneInstance maxPort = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, 65535, REGISTERED_AT, LAST_HEARTBEAT);
        assertEquals(65535, maxPort.grpcPort());
    }

    @Test
    void validConstructionWithZeroTimestamps() {
        ControlPlaneInstance instance = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, 0, 0);

        assertEquals(0, instance.registeredAt());
        assertEquals(0, instance.lastHeartbeat());
    }

    @Test
    void withHeartbeatReturnsNewInstanceWithUpdatedTimestamp() {
        ControlPlaneInstance original = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT);

        long newHeartbeat = LAST_HEARTBEAT + 5000;
        ControlPlaneInstance updated = original.withHeartbeat(newHeartbeat);

        // Must be a distinct object
        assertNotSame(original, updated);

        // Only lastHeartbeat should change
        assertEquals(INSTANCE_ID, updated.instanceId());
        assertEquals(REGION, updated.region());
        assertEquals(GRPC_ADDRESS, updated.grpcAddress());
        assertEquals(GRPC_PORT, updated.grpcPort());
        assertEquals(REGISTERED_AT, updated.registeredAt());
        assertEquals(newHeartbeat, updated.lastHeartbeat());

        // Original must be unchanged
        assertEquals(LAST_HEARTBEAT, original.lastHeartbeat());
    }

    // ---- Null field validation ----

    @Test
    void nullInstanceIdThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () -> new ControlPlaneInstance(
                null, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void nullRegionThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, null, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void nullGrpcAddressThrowsNullPointerException() {
        assertThrows(NullPointerException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, null, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    // ---- Blank field validation ----

    @Test
    void blankInstanceIdThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                "", REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                "   ", REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void blankRegionThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, "", GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, "   ", GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void blankGrpcAddressThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, "", GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, "   ", GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT));
    }

    // ---- Port range validation ----

    @Test
    void grpcPortZeroThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, 0, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void grpcPortNegativeThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, -1, REGISTERED_AT, LAST_HEARTBEAT));
    }

    @Test
    void grpcPortAbove65535ThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, 65536, REGISTERED_AT, LAST_HEARTBEAT));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, 100_000, REGISTERED_AT, LAST_HEARTBEAT));
    }

    // ---- Timestamp validation ----

    @Test
    void negativeRegisteredAtThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, -1, LAST_HEARTBEAT));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, Long.MIN_VALUE, LAST_HEARTBEAT));
    }

    @Test
    void negativeLastHeartbeatThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, -1));

        assertThrows(IllegalArgumentException.class, () -> new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, Long.MIN_VALUE));
    }

    @Test
    void withHeartbeatNegativeValueThrowsIllegalArgumentException() {
        ControlPlaneInstance instance = new ControlPlaneInstance(
                INSTANCE_ID, REGION, GRPC_ADDRESS, GRPC_PORT, REGISTERED_AT, LAST_HEARTBEAT);

        assertThrows(IllegalArgumentException.class, () -> instance.withHeartbeat(-1));
    }
}
