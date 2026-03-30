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
package com.shieldblaze.expressgateway.servicediscovery.server;

import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class BulkRequestTest {

    @Test
    void validBulkRequest() {
        BulkRequest request = new BulkRequest(
                List.of(
                        new Node("svc-1", "10.0.0.1", 8080, false),
                        new Node("svc-2", "10.0.0.2", 8081, true)
                ),
                60
        );

        assertDoesNotThrow(request::validate);
        assertEquals(2, request.nodes().size());
        assertEquals(60, request.ttlSeconds());
    }

    @Test
    void emptyNodesThrows() {
        BulkRequest request = new BulkRequest(List.of(), 60);
        assertThrows(IllegalArgumentException.class, request::validate);
    }

    @Test
    void nullNodesThrows() {
        BulkRequest request = new BulkRequest(null, 60);
        assertThrows(IllegalArgumentException.class, request::validate);
    }

    @Test
    void negativeTtlThrows() {
        BulkRequest request = new BulkRequest(
                List.of(new Node("svc-1", "10.0.0.1", 8080, false)),
                -1
        );
        assertThrows(IllegalArgumentException.class, request::validate);
    }

    @Test
    void invalidNodeInBulkThrows() {
        BulkRequest request = new BulkRequest(
                List.of(new Node(null, "10.0.0.1", 8080, false)),
                60
        );
        assertThrows(NullPointerException.class, request::validate);
    }
}
