package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertThrows;

class L4BalanceTest {

    @Test
    void getBackend() {
        Backend backend = new Backend(new InetSocketAddress("192.168.1.1", 9110));

        L4Balance l4Balance = new EmptyL4Balance();
        l4Balance.setBackends(Collections.singletonList(backend));

        Assertions.assertEquals(backend, l4Balance.getBackend(null));
    }

    @Test
    void throwsException() {
        assertThrows(NullPointerException.class, () -> new EmptyL4Balance().setBackends(null));
        assertThrows(IllegalArgumentException.class, () -> new EmptyL4Balance().setBackends(Collections.emptyList()));
    }

    private static final class EmptyL4Balance extends L4Balance {

        public EmptyL4Balance() {
            super(new NOOPSessionPersistence());
        }

        @Override
        public Backend getBackend(InetSocketAddress sourceAddress) {
            return backends.get(0);
        }
    }
}
