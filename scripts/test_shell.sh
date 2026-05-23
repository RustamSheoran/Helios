# Helios Shell Test Script
echo "--- Starting Helios Shell Test ---"
echo "Testing environment variable expansion..."
export TEST_VAR=HeliosIsAwesome
echo "TEST_VAR value is: $TEST_VAR"
echo "Testing directory change..."
cd /home/rustam/dist/helios
echo "Testing pipelines and redirects..."
ls -la | grep Cargo > cargo_files.txt
cat cargo_files.txt
echo "Cleaning up..."
rm -f cargo_files.txt
echo "--- Shell Test Completed Successfully ---"
