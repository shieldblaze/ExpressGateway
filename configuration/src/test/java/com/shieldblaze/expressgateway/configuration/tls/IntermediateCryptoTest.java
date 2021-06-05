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

import org.junit.jupiter.api.Test;

import java.util.List;

import static org.hamcrest.CoreMatchers.is;
import static org.hamcrest.MatcherAssert.assertThat;

class IntermediateCryptoTest {

    @Test
    void simpleProtocolTest() {
        List<Protocol> protocols = List.of(Protocol.TLS_1_3, Protocol.TLS_1_2);
        assertThat(protocols, is(IntermediateCrypto.PROTOCOLS));
    }

    @Test
    void simpleCipherTest() {
        List<Cipher> ciphers = List.of(
                Cipher.TLS_AES_256_GCM_SHA384,
                Cipher.TLS_AES_128_GCM_SHA256,
                Cipher.TLS_CHACHA20_POLY1305_SHA256,
                Cipher.TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
                Cipher.TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
                Cipher.TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
                Cipher.TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
                Cipher.TLS_DHE_RSA_WITH_AES_256_GCM_SHA384,
                Cipher.TLS_DHE_RSA_WITH_AES_128_GCM_SHA256
        );

        assertThat(ciphers, is(IntermediateCrypto.CIPHERS));
    }
}
