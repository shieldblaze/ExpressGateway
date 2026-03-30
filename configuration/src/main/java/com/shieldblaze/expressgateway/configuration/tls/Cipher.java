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
package com.shieldblaze.expressgateway.configuration.tls;

/**
 * List of available Ciphers Suites under OpenSsl 1.1.1g.
 *
 * <p><b>Security notes:</b></p>
 * <ul>
 *     <li>{@code TLS_DH_anon_*} and {@code TLS_ECDH_anon_*} ciphers provide no authentication
 *         and are vulnerable to trivial MITM attacks. They are rejected in PRODUCTION.</li>
 *     <li>{@code TLS_RSA_WITH_*} (non-ECDHE/DHE) ciphers do not provide forward secrecy.
 *         They are rejected in PRODUCTION.</li>
 *     <li>{@code TLS_SRP_*} ciphers are rejected in PRODUCTION.</li>
 *     <li>{@code *_CBC_*} ciphers are deprecated due to padding oracle vulnerabilities
 *         (POODLE variants). Prefer GCM or ChaCha20 ciphers.</li>
 * </ul>
 */
public enum Cipher {
    TLS_AES_256_GCM_SHA384,
    TLS_CHACHA20_POLY1305_SHA256,
    TLS_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_DHE_DSS_WITH_AES_256_GCM_SHA384,
    TLS_DHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_CCM8,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_CCM,
    TLS_DHE_RSA_WITH_AES_256_CBC_CCM8,
    TLS_DHE_RSA_WITH_AES_256_CBC_CCM,
    TLS_ECDHE_ECDSA_WITH_ARIA256_GCM_SHA384,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_ECDHE_ARIA256_GCM_SHA384,
    TLS_DHE_DSS_WITH_ARIA256_GCM_SHA384,
    TLS_DHE_RSA_WITH_ARIA256_GCM_SHA384,
    /** @deprecated Anonymous DH -- no authentication, vulnerable to MITM. Rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_DHE_DSS_WITH_AES_128_GCM_SHA256,
    TLS_DHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_CCM8,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_CCM,
    TLS_DHE_RSA_WITH_AES_128_CBC_CCM8,
    TLS_DHE_RSA_WITH_AES_128_CBC_CCM,
    TLS_ECDHE_ECDSA_WITH_ARIA128_GCM_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_ECDHE_ARIA128_GCM_SHA256,
    TLS_DHE_DSS_WITH_ARIA128_GCM_SHA256,
    TLS_DHE_RSA_WITH_ARIA128_GCM_SHA256,
    /** @deprecated Anonymous DH -- no authentication, vulnerable to MITM. Rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_128_GCM_SHA256,
    /** Deprecated: CBC cipher suite -- vulnerable to padding oracle attacks (POODLE variants). */
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384,
    TLS_DHE_RSA_WITH_AES_256_CBC_SHA256,
    TLS_DHE_DSS_WITH_AES_256_CBC_SHA256,
    TLS_ECDHE_ECDSA_WITH_CAMELLIA256_SHA384,
    TLS_ECDHE_RSA_WITH_CAMELLIA256_SHA384,
    TLS_DHE_RSA_WITH_CAMELLIA256_SHA256,
    TLS_DHE_DSS_WITH_CAMELLIA256_SHA256,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_256_CBC_SHA256,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_CAMELLIA256_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256,
    TLS_DHE_RSA_WITH_AES_128_CBC_SHA256,
    TLS_DHE_DSS_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_ECDSA_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_RSA_WITH_CAMELLIA128_SHA256,
    TLS_DHE_RSA_WITH_CAMELLIA128_SHA256,
    TLS_DHE_DSS_WITH_CAMELLIA128_SHA256,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_128_CBC_SHA256,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_DHE_DSS_WITH_AES_256_CBC_SHA,
    TLS_DHE_RSA_WITH_CAMELLIA256_SHA,
    TLS_DHE_DSS_WITH_CAMELLIA256_SHA,
    /** @deprecated Anonymous ECDH -- no authentication, vulnerable to MITM. Rejected in PRODUCTION. */
    @Deprecated
    TLS_ECDH_anon_WITH_AES_256_CBC_SHA,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_256_CBC_SHA,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_CAMELLIA256_SHA,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_DHE_DSS_WITH_AES_128_CBC_SHA,
    TLS_DHE_RSA_WITH_SEED_SHA,
    TLS_DHE_DSS_WITH_SEED_SHA,
    TLS_DHE_RSA_WITH_CAMELLIA128_SHA,
    TLS_DHE_DSS_WITH_CAMELLIA128_SHA,
    /** @deprecated Anonymous ECDH -- no authentication, vulnerable to MITM. Rejected in PRODUCTION. */
    @Deprecated
    TLS_ECDH_anon_WITH_AES_128_CBC_SHA,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_AES_128_CBC_SHA,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_SEED_SHA,
    /** @deprecated Anonymous DH -- rejected in PRODUCTION. */
    @Deprecated
    TLS_DH_anon_WITH_CAMELLIA128_SHA,
    TLS_RSA_PSK_WITH_AES_256_GCM_SHA384,
    TLS_DHE_PSK_WITH_AES_256_GCM_SHA384,
    TLS_RSA_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_PSK_WITH_AES_256_CBC_CCM8,
    TLS_DHE_PSK_WITH_AES_256_CBC_CCM,
    TLS_RSA_PSK_WITH_ARIA256_GCM_SHA384,
    TLS_DHE_PSK_WITH_ARIA256_GCM_SHA384,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_256_GCM_SHA384,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_256_CBC_CCM8,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_256_CBC_CCM,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_ARIA256_GCM_SHA384,
    TLS_PSK_WITH_AES_256_GCM_SHA384,
    TLS_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_PSK_WITH_AES_256_CBC_CCM8,
    TLS_PSK_WITH_AES_256_CBC_CCM,
    TLS_PSK_WITH_ARIA256_GCM_SHA384,
    TLS_RSA_PSK_WITH_AES_128_GCM_SHA256,
    TLS_DHE_PSK_WITH_AES_128_GCM_SHA256,
    TLS_DHE_PSK_WITH_AES_128_CBC_CCM8,
    TLS_DHE_PSK_WITH_AES_128_CBC_CCM,
    TLS_RSA_PSK_WITH_ARIA128_GCM_SHA256,
    TLS_DHE_PSK_WITH_ARIA128_GCM_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_128_GCM_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_128_CBC_CCM8,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_128_CBC_CCM,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_ARIA128_GCM_SHA256,
    TLS_PSK_WITH_AES_128_GCM_SHA256,
    TLS_PSK_WITH_AES_128_CBC_CCM8,
    TLS_PSK_WITH_AES_128_CBC_CCM,
    TLS_PSK_WITH_ARIA128_GCM_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_256_CBC_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_CAMELLIA256_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_128_CBC_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_PSK_WITH_AES_256_CBC_SHA384,
    TLS_ECDHE_PSK_WITH_AES_256_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_DSS_WITH_AES_256_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_RSA_WITH_AES_256_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_WITH_AES_256_CBC_SHA,
    TLS_RSA_PSK_WITH_AES_256_CBC_SHA384,
    TLS_DHE_PSK_WITH_AES_256_CBC_SHA384,
    TLS_RSA_PSK_WITH_AES_256_CBC_SHA,
    TLS_DHE_PSK_WITH_AES_256_CBC_SHA,
    TLS_ECDHE_PSK_WITH_CAMELLIA256_SHA384,
    TLS_RSA_PSK_WITH_CAMELLIA256_SHA384,
    TLS_DHE_PSK_WITH_CAMELLIA256_SHA384,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_256_CBC_SHA,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_CAMELLIA256_SHA,
    TLS_PSK_WITH_AES_256_CBC_SHA384,
    TLS_PSK_WITH_AES_256_CBC_SHA,
    TLS_PSK_WITH_CAMELLIA256_SHA384,
    TLS_ECDHE_PSK_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_PSK_WITH_AES_128_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_DSS_WITH_AES_128_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_RSA_WITH_AES_128_CBC_SHA,
    /** @deprecated SRP cipher -- rejected in PRODUCTION. */
    @Deprecated
    TLS_SRP_WITH_AES_128_CBC_SHA,
    TLS_RSA_PSK_WITH_AES_128_CBC_SHA256,
    TLS_DHE_PSK_WITH_AES_128_CBC_SHA256,
    TLS_RSA_PSK_WITH_AES_128_CBC_SHA,
    TLS_DHE_PSK_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_PSK_WITH_CAMELLIA128_SHA256,
    TLS_RSA_PSK_WITH_CAMELLIA128_SHA256,
    TLS_DHE_PSK_WITH_CAMELLIA128_SHA256,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_AES_128_CBC_SHA,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_SEED_SHA,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_CAMELLIA128_SHA,
    /** @deprecated No forward secrecy; use ECDHE or DHE key exchange instead. */
    @Deprecated
    TLS_RSA_WITH_IDEA_CBC_SHA,
    TLS_PSK_WITH_AES_128_CBC_SHA256,
    TLS_PSK_WITH_AES_128_CBC_SHA,
    TLS_PSK_WITH_CAMELLIA128_SHA256;

    /**
     * TLS-F5: Check whether this cipher is deprecated (anonymous DH/ECDH, non-PFS RSA, SRP).
     *
     * <p>Uses reflection to inspect the enum constant's field-level {@link Deprecated}
     * annotation. This is safe for enums because the public static field name always
     * matches {@link #name()} and the annotation is retained at runtime by default.</p>
     *
     * @return {@code true} if this cipher suite is marked {@code @Deprecated}
     */
    public boolean isDeprecated() {
        try {
            return getClass().getField(name()).isAnnotationPresent(Deprecated.class);
        } catch (NoSuchFieldException e) {
            // Should never happen for an enum constant
            return false;
        }
    }
}
