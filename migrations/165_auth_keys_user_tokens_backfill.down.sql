-- Deliberately empty. The up migration only adds mirror rows that the writers
-- would have created anyway, and the rows carry no marker distinguishing them
-- from a normally-minted key — deleting by guess would revoke live credentials.
SELECT 1;
