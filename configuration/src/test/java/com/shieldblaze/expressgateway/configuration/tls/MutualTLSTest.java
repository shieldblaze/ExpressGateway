/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

package com.shieldblaze.expressgateway.configuration.tls;

import io.netty.handler.ssl.ClientAuth;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class MutualTLSTest {

    @Test
    void simpleTest() {
        assertEquals(ClientAuth.NONE, MutualTLS.NOT_REQUIRED.clientAuth());
        assertEquals(ClientAuth.OPTIONAL, MutualTLS.OPTIONAL.clientAuth());
        assertEquals(ClientAuth.REQUIRE, MutualTLS.REQUIRED.clientAuth());
    }
}
