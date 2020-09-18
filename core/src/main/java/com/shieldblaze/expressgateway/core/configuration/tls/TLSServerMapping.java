/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.configuration.tls;

import java.util.HashMap;
import java.util.Map;

public final class TLSServerMapping {
    final Map<String, ServerCertificateKey> certificateKeyMap = new HashMap<>();
    final boolean hasDefaultMapping;

    public TLSServerMapping() {
        this(false);
    }

    public TLSServerMapping(boolean hasDefaultMapping) {
        this.hasDefaultMapping = hasDefaultMapping;
    }

    public void addMapping(String hostname, ServerCertificateKey serverCertificateKey) {
        certificateKeyMap.put(hostname, serverCertificateKey);
    }

    public void addDefaultHost(ServerCertificateKey serverCertificateKey) {
        certificateKeyMap.put("DEFAULT_HOST", serverCertificateKey);
    }
}
