/*
 * Copyright 2020 Alibaba Group Holding Limited.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package com.alibaba.graphscope.common.manager;

import com.alibaba.graphscope.common.config.Configs;
import com.alibaba.graphscope.common.config.FrontendConfig;

import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.time.Duration;
import java.util.concurrent.*;

public class RateLimitExecutor extends ThreadPoolExecutor {
    private static final Logger logger = LoggerFactory.getLogger(RateLimitExecutor.class);
    private final Bucket bucket;

    public RateLimitExecutor(
            Configs configs,
            int corePoolSize,
            int maximumPoolSize,
            long keepAliveTime,
            TimeUnit unit,
            BlockingQueue<Runnable> workQueue,
            ThreadFactory threadFactory,
            RejectedExecutionHandler handler) {
        super(
                corePoolSize,
                maximumPoolSize,
                keepAliveTime,
                unit,
                workQueue,
                threadFactory,
                handler);

        int permitsPerSecond = FrontendConfig.QUERY_PER_SECOND_LIMIT.get(configs);
        int permitsPerMilli = permitsPerSecond / 200;
        permitsPerMilli = permitsPerMilli == 0 ? 1 : permitsPerMilli;
        int permitsPerMilli2 = permitsPerSecond / 100;
        permitsPerMilli = permitsPerMilli == 0 ? 1 : permitsPerMilli;
        logger.info(
                "Configured rate limiter: {}/s, {}/5ms, {}/10ms",
                permitsPerSecond,
                permitsPerMilli,
                permitsPerMilli2);
        Bandwidth second = Bandwidth.simple(permitsPerSecond, Duration.ofSeconds(1));
        Bandwidth millis = Bandwidth.simple(permitsPerMilli, Duration.ofMillis(5));
        Bandwidth millis2 = Bandwidth.simple(permitsPerMilli2, Duration.ofMillis(10));
        this.bucket = Bucket.builder().addLimit(second).addLimit(millis).addLimit(millis2).build();
    }

    public Future<?> submit(Runnable task) {
        try {
            if (bucket.asBlocking().tryConsume(1, Duration.ofMillis(500))) {
                return super.submit(task);
            }
        } catch (InterruptedException ignored) {

        }
        throw new RejectedExecutionException(
                "rate limit exceeded, Please increase the QPS limit by the config"
                        + " 'query.per.second.limit' or slow down the query sending speed");
    }
}
