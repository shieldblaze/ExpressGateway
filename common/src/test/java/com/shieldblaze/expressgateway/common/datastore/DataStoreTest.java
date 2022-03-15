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
package com.shieldblaze.expressgateway.common.datastore;

import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class DataStoreTest {

    @Order(1)
    @Test
    void storeAndGetTest() throws Exception {
        char[] password = "meow".toCharArray();
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"));
        DataStore.INSTANCE.store(password, "Cat", ssc.keyPair().getPrivate(), ssc.x509Certificate());

        Entry entry = DataStore.INSTANCE.get(password, "Cat");
        assertArrayEquals(ssc.keyPair().getPrivate().getEncoded(), entry.privateKey().getEncoded());
        assertArrayEquals(ssc.x509Certificate().getEncoded(), entry.certificates()[0].getEncoded());
    }

    @Order(2)
    @Test
    void getEntireTest() throws IOException {
        assertTrue(DataStore.INSTANCE.getEntire().length > 0);
    }

    @Order(3)
    @Test
    void destroyTest() {
        assertTrue(DataStore.INSTANCE.destroy());
    }
}
