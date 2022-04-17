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

package com.shieldblaze.expressgateway.restapi;

import com.shieldblaze.expressgateway.common.datastore.DataStore;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;

import java.util.List;

public final class Utils {

    private Utils() {
        // Prevent outside initialization
    }

    private static final SelfSignedCertificate selfSignedCertificate =
            SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("localhost"));

    public static void initSelfSignedDataStore() {
        final String password = "MeowMeowCatCat";
        final String alias = "meowAlias";

        DataStore.INSTANCE.store(password.toCharArray(), alias, selfSignedCertificate.keyPair().getPrivate(), selfSignedCertificate.x509Certificate());
        System.setProperty("datastore.alias", alias);
        System.setProperty("datastore.password", password);
    }

    public static SelfSignedCertificate selfSignedCertificate() {
        return selfSignedCertificate;
    }
}
