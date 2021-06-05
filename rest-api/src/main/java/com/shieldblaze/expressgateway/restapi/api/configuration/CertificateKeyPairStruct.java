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
package com.shieldblaze.expressgateway.restapi.api.configuration;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.crypto.CertificateUtil;
import com.shieldblaze.expressgateway.common.crypto.PrivateKeyUtil;
import com.shieldblaze.expressgateway.common.utils.ListUtil;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

public final class CertificateKeyPairStruct {

    @JsonIgnore
    private final List<X509Certificate> x509Certificates = new ArrayList<>();

    @JsonIgnore
    private PrivateKey privateKey;

    @JsonProperty("host")
    private String host;

    @JsonProperty("useOCSP")
    private boolean useOCSP;

    @JsonProperty("certificates")
    private List<String> certificates;

    @JsonProperty("privateKey")
    private String privateKeyAsString;

    public void setCertificates(List<String> certificates) {
        ListUtil.checkNonEmpty(certificates, "Certificates");
        for (String cert : certificates) {
            x509Certificates.add(CertificateUtil.parseX509Certificate(cert));
        }
        this.certificates = certificates;
    }

    public void setPrivateKeyAsString(String privateKey) {
        this.privateKeyAsString = Objects.requireNonNull(privateKey, "PrivateKey");
        this.privateKey = PrivateKeyUtil.parsePrivateKey(privateKey);
    }

    public List<X509Certificate> x509Certificates() {
        return x509Certificates;
    }

    public PrivateKey privateKey() {
        return privateKey;
    }

    public void setHost(String host) {
        this.host = Objects.requireNonNull(host, "host");
    }

    public void setUseOCSP(boolean useOCSP) {
        this.useOCSP = useOCSP;
    }

    public String host() {
        return host;
    }

    public boolean useOCSP() {
        return useOCSP;
    }
}
