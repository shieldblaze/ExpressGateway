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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NodeTest {

    @Test
    void validateNodeTest() {
        // ID will be null
        assertThrows(NullPointerException.class, () -> new Node().validate());

        // Invalid IP Address
        assertThrows(IllegalArgumentException.class, () -> new Node("1-2-3-4-5-f", "300.300.300.300", 9110, false).validate());

        // Invalid Port
        assertThrows(IllegalArgumentException.class, () -> new Node("1-2-3-4-5-f", "127.0.0.1", 91100, false).validate());

        assertDoesNotThrow(() -> new Node("1-2-3-4-5-f", "127.0.0.1", 9110, false).validate());
    }
}
