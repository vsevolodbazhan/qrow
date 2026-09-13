package io.qrow.fixture;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import org.apache.hadoop.hive.ql.exec.UDF;
import org.apache.spark.SparkEnv;
import org.apache.spark.TaskContext;
import org.apache.spark.scheduler.SparkListener;
import org.apache.spark.scheduler.SparkListenerApplicationStart;
import org.apache.spark.scheduler.SparkListenerTaskEnd;

/** Executor evidence for cancellation and SQL replay tests. Never shipped with Qrow. */
public final class Blocking extends UDF {
    private static void record(String token, String state) throws java.io.IOException {
        if (!token.matches("[a-zA-Z0-9_-]+")) {
            throw new IllegalArgumentException("Invalid evidence token");
        }
        Files.writeString(Path.of("/evidence", token + "." + state), "1\n",
            StandardOpenOption.CREATE, StandardOpenOption.APPEND);
    }

    /** Driver-side terminal task evidence, independent of the client's cancellation response. */
    public static final class CompletionListener extends SparkListener {
        private String application;
        @Override public void onApplicationStart(SparkListenerApplicationStart event) {
            application = event.appId().get();
        }
        @Override public void onTaskEnd(SparkListenerTaskEnd event) {
            try {
                record(application + "-" + event.taskInfo().taskId(), "ended");
            } catch (java.io.IOException error) {
                throw new java.io.UncheckedIOException(error);
            }
        }
    }

    public Long evaluate(Long value, String token, Long milliseconds) throws Exception {
        String task = SparkEnv.get().conf().get("spark.app.id") + "-" + TaskContext.get().taskAttemptId();
        Files.writeString(Path.of("/evidence", token + ".task"), task);
        record(token, "started");
        try {
            Thread.sleep(milliseconds);
            record(token, "completed");
            return value;
        } catch (InterruptedException interrupted) {
            record(token, "interrupted");
            Thread.currentThread().interrupt();
            throw interrupted;
        }
    }
}
