import sys
import matplotlib.pyplot as plt
from web3 import Web3

RPC_URL = "https://maximum-yolo-seed.quiknode.pro/e4a602e14006c812850883f288b1574b36c48ef6"

def get_block_data(w3, block_number):
    """Query block data from RPC."""
    try:
        block = w3.eth.get_block(block_number, full_transactions=False)
        tx_count = len(block.transactions) if block.transactions else 0
        gas_used = block.gasUsed
        # Base fee is available in EIP-1559 blocks (post-London fork)
        base_fee = block.baseFeePerGas if hasattr(block, 'baseFeePerGas') else None
        return tx_count, gas_used, base_fee
    except Exception as e:
        print(f"Error querying block {block_number}: {e}")
        return None, None, None

def main():
    if len(sys.argv) < 2:
        print("Usage: python plot_txs.py <initial_block_number> [last_block_number]")
        sys.exit(1)

    initial_block = int(sys.argv[1])

    # Connect to RPC
    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    if not w3.is_connected():
        print(f"Failed to connect to RPC at {RPC_URL}")
        sys.exit(1)

    # Get last block number (use provided argument or query latest)
    if len(sys.argv) >= 3:
        last_block = int(sys.argv[2])
        print(f"Using provided last block: {last_block}")
    else:
        last_block = w3.eth.block_number
        print(f"Using latest block from RPC: {last_block}")

    print(f"Initial block: {initial_block}")
    print(f"Last block: {last_block}")
    print(f"Querying {last_block - initial_block + 1} blocks...")

    # Collect data
    block_numbers = []
    tx_counts = []
    gas_used_list = []
    base_fees = []

    for block_num in range(initial_block, last_block + 1):
        if (block_num - initial_block) % 100 == 0:
            print(f"Processing block {block_num}...")

        tx_count, gas_used, base_fee = get_block_data(w3, block_num)

        if tx_count is not None:
            block_numbers.append(block_num)
            tx_counts.append(tx_count)
            gas_used_list.append(gas_used)
            if base_fee is not None:
                base_fees.append(base_fee)
            else:
                base_fees.append(0)  # Pre-EIP-1559 blocks don't have base fee

    print(f"Collected data for {len(block_numbers)} blocks")

    # Save raw data for all 4 charts to CSV files

    # Chart 1: Transaction count (line chart)
    with open("tx_count.csv", "w") as f:
        f.write("block_number,tx_count\n")
        for block_num, tx_count in zip(block_numbers, tx_counts):
            f.write(f"{block_num},{tx_count}\n")
    print(f"Transaction count data written to tx_count.csv")

    # Chart 2: Transaction count histogram (frequency distribution)
    from collections import Counter
    tx_frequency = Counter(tx_counts)
    max_tx = max(tx_counts) if tx_counts else 0
    with open("tx_histogram.csv", "w") as f:
        f.write("tx_count,number_of_blocks\n")
        for tx_count in range(0, max_tx + 1):
            num_blocks = tx_frequency.get(tx_count, 0)
            f.write(f"{tx_count},{num_blocks}\n")
    print(f"Transaction histogram data written to tx_histogram.csv (0 to {max_tx} transactions)")

    # Chart 3: Gas used (line chart)
    with open("gas_used.csv", "w") as f:
        f.write("block_number,gas_used\n")
        for block_num, gas_used in zip(block_numbers, gas_used_list):
            f.write(f"{block_num},{gas_used}\n")
    print(f"Gas used data written to gas_used.csv")

    # Chart 4: Base fee (line chart)
    with open("base_fee.csv", "w") as f:
        f.write("block_number,base_fee\n")
        for block_num, base_fee in zip(block_numbers, base_fees):
            f.write(f"{block_num},{base_fee}\n")
    print(f"Base fee data written to base_fee.csv")

    # Create subplots - 4 plots: 3 line charts + 1 histogram
    # fig, axes = plt.subplots(2, 2, figsize=(14, 10))

    # # Plot 1: Transaction count (line chart)
    # axes[0, 0].plot(block_numbers, tx_counts, linewidth=1, color='blue')
    # axes[0, 0].set_xlabel("Block number")
    # axes[0, 0].set_ylabel("Transactions per block")
    # axes[0, 0].set_title("Transactions per Block (Line Chart)")
    # axes[0, 0].grid(True, alpha=0.3)

    # # Plot 2: Transaction count (histogram)
    # axes[0, 1].hist(tx_counts, bins=50, color='blue', alpha=0.7, edgecolor='black')
    # axes[0, 1].set_xlabel("Transactions per block")
    # axes[0, 1].set_ylabel("Frequency")
    # axes[0, 1].set_title("Transaction Count Distribution (Histogram)")
    # axes[0, 1].grid(True, alpha=0.3, axis='y')

    # # Plot 3: Gas used
    # axes[1, 0].plot(block_numbers, gas_used_list, linewidth=1, color='green')
    # axes[1, 0].set_xlabel("Block number")
    # axes[1, 0].set_ylabel("Gas used")
    # axes[1, 0].set_title("Block Gas Used")
    # axes[1, 0].grid(True, alpha=0.3)

    # # Plot 4: Base fee
    # axes[1, 1].plot(block_numbers, base_fees, linewidth=1, color='red')
    # axes[1, 1].set_xlabel("Block number")
    # axes[1, 1].set_ylabel("Base fee (wei)")
    # axes[1, 1].set_title("Block Base Fee")
    # axes[1, 1].grid(True, alpha=0.3)

    # plt.tight_layout()
    # plt.show()

if __name__ == "__main__":
    main()
