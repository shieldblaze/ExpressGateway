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
package com.shieldblaze.expressgateway.common.crypto.cryptostore;

import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import org.junit.jupiter.api.Test;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;

class CryptoStoreTest {

    @Test
    void storeAndFetchTest() throws Exception {
        char[] password = "meow" .toCharArray();
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("shieldblaze.com"));

        try (ByteArrayOutputStream outputStream = new ByteArrayOutputStream()) {
            CryptoStore.storePrivateKeyAndCertificate(null, outputStream, password, "Cat", ssc.keyPair().getPrivate(), ssc.x509Certificate());

            try (ByteArrayInputStream inputStream = new ByteArrayInputStream(outputStream.toByteArray())) {

                CryptoEntry cryptoEntry = CryptoStore.fetchPrivateKeyCertificateEntry(inputStream, password, "Cat");
                assertArrayEquals(ssc.keyPair().getPrivate().getEncoded(), cryptoEntry.privateKey().getEncoded());
                assertArrayEquals(ssc.x509Certificate().getEncoded(), cryptoEntry.certificates()[0].getEncoded());
            }
        }
    }
}
