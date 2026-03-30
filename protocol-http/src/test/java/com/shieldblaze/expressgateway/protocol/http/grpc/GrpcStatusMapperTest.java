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
package com.shieldblaze.expressgateway.protocol.http.grpc;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class GrpcStatusMapperTest {

    @Test
    void testCanonicalHttpToGrpcMapping() {
        assertEquals(GrpcStatusMapper.OK, GrpcStatusMapper.httpToGrpcStatus(200));
        assertEquals(GrpcStatusMapper.INVALID_ARGUMENT, GrpcStatusMapper.httpToGrpcStatus(400));
        assertEquals(GrpcStatusMapper.UNAUTHENTICATED, GrpcStatusMapper.httpToGrpcStatus(401));
        assertEquals(GrpcStatusMapper.PERMISSION_DENIED, GrpcStatusMapper.httpToGrpcStatus(403));
        assertEquals(GrpcStatusMapper.UNIMPLEMENTED, GrpcStatusMapper.httpToGrpcStatus(404));
        assertEquals(GrpcStatusMapper.DEADLINE_EXCEEDED, GrpcStatusMapper.httpToGrpcStatus(408));
        assertEquals(GrpcStatusMapper.ABORTED, GrpcStatusMapper.httpToGrpcStatus(409));
        assertEquals(GrpcStatusMapper.RESOURCE_EXHAUSTED, GrpcStatusMapper.httpToGrpcStatus(429));
        assertEquals(GrpcStatusMapper.CANCELLED, GrpcStatusMapper.httpToGrpcStatus(499));
        assertEquals(GrpcStatusMapper.INTERNAL, GrpcStatusMapper.httpToGrpcStatus(500));
        assertEquals(GrpcStatusMapper.UNIMPLEMENTED, GrpcStatusMapper.httpToGrpcStatus(501));
        assertEquals(GrpcStatusMapper.UNAVAILABLE, GrpcStatusMapper.httpToGrpcStatus(502));
        assertEquals(GrpcStatusMapper.UNAVAILABLE, GrpcStatusMapper.httpToGrpcStatus(503));
        assertEquals(GrpcStatusMapper.DEADLINE_EXCEEDED, GrpcStatusMapper.httpToGrpcStatus(504));
    }

    @Test
    void testUnmappedHttpStatusReturnsUnknown() {
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(201));
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(204));
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(301));
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(418)); // I'm a teapot
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(0));
        assertEquals(GrpcStatusMapper.UNKNOWN, GrpcStatusMapper.httpToGrpcStatus(-1));
    }

    @Test
    void testGrpcStatusNames() {
        assertEquals("OK", GrpcStatusMapper.grpcStatusName(0));
        assertEquals("CANCELLED", GrpcStatusMapper.grpcStatusName(1));
        assertEquals("UNKNOWN", GrpcStatusMapper.grpcStatusName(2));
        assertEquals("INVALID_ARGUMENT", GrpcStatusMapper.grpcStatusName(3));
        assertEquals("DEADLINE_EXCEEDED", GrpcStatusMapper.grpcStatusName(4));
        assertEquals("NOT_FOUND", GrpcStatusMapper.grpcStatusName(5));
        assertEquals("ALREADY_EXISTS", GrpcStatusMapper.grpcStatusName(6));
        assertEquals("PERMISSION_DENIED", GrpcStatusMapper.grpcStatusName(7));
        assertEquals("RESOURCE_EXHAUSTED", GrpcStatusMapper.grpcStatusName(8));
        assertEquals("FAILED_PRECONDITION", GrpcStatusMapper.grpcStatusName(9));
        assertEquals("ABORTED", GrpcStatusMapper.grpcStatusName(10));
        assertEquals("OUT_OF_RANGE", GrpcStatusMapper.grpcStatusName(11));
        assertEquals("UNIMPLEMENTED", GrpcStatusMapper.grpcStatusName(12));
        assertEquals("INTERNAL", GrpcStatusMapper.grpcStatusName(13));
        assertEquals("UNAVAILABLE", GrpcStatusMapper.grpcStatusName(14));
        assertEquals("DATA_LOSS", GrpcStatusMapper.grpcStatusName(15));
        assertEquals("UNAUTHENTICATED", GrpcStatusMapper.grpcStatusName(16));
    }

    @Test
    void testOutOfRangeStatusCodeReturnsUnknown() {
        assertEquals("UNKNOWN", GrpcStatusMapper.grpcStatusName(-1));
        assertEquals("UNKNOWN", GrpcStatusMapper.grpcStatusName(17));
        assertEquals("UNKNOWN", GrpcStatusMapper.grpcStatusName(100));
    }

    @Test
    void testProxyRelevantMappings() {
        // These are the critical mappings a proxy generates:
        // 502 Bad Gateway -> UNAVAILABLE (backend connection failed)
        assertEquals(GrpcStatusMapper.UNAVAILABLE, GrpcStatusMapper.httpToGrpcStatus(502));
        // 503 Service Unavailable -> UNAVAILABLE (no backend available)
        assertEquals(GrpcStatusMapper.UNAVAILABLE, GrpcStatusMapper.httpToGrpcStatus(503));
        // 504 Gateway Timeout -> DEADLINE_EXCEEDED (backend timeout)
        assertEquals(GrpcStatusMapper.DEADLINE_EXCEEDED, GrpcStatusMapper.httpToGrpcStatus(504));
    }
}
