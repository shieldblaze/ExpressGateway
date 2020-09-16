package com.shieldblaze.expressgateway.core.configuration.tls;

import java.util.ArrayList;
import java.util.List;

public final class TLSConfiguration {
    private final List<Ciphers> ciphers = new ArrayList<>();
    private final List<Protocols> protocols = new ArrayList<>();
}
